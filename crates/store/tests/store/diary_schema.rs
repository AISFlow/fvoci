//! The migration of the diaries (55) on a database a server left before it, and what
//! the reads of a full diary cost: both say what they measured where the tests are run
//! with their output shown (`--nocapture`).

use std::time::{Duration, Instant};

use fvoci_model::{
    Container, ContainerKind, ContainerVisibility, DiaryDate, DiaryMaking, ENTRIES_PER_DIARY,
    MonthDay, NewDiaryEntry, new_id,
};
use fvoci_store::{DbType, Store};

use crate::boards::new_board;
use crate::common::{self, db_type};

/// The columns of a table, in their order.
async fn columns_of(system: &Store, table: &str) -> Vec<String> {
    let query = match db_type() {
        DbType::Sqlite => format!("SELECT name FROM pragma_table_info('{table}') ORDER BY cid"),
        DbType::Postgres => format!(
            "SELECT column_name::text FROM information_schema.columns \
             WHERE table_schema = current_schema() AND table_name = '{table}' \
             ORDER BY ordinal_position"
        ),
    };
    system.texts_unchecked(&query).await.expect("read")
}

/// A diary of the space, by the store.
async fn a_diary(own: &Store, workspace_id: &str, user_id: &str) -> String {
    let diary = Container {
        kind: ContainerKind::Diary,
        ..new_board(&new_id(), workspace_id, ContainerVisibility::Private)
    };
    let made = own.create_diary(diary, user_id, DiaryMaking::Another).await;
    made.expect("made").diary.id
}

fn day(text: &str) -> DiaryDate {
    text.parse().expect("a day")
}

fn of_day(date: DiaryDate) -> NewDiaryEntry {
    NewDiaryEntry {
        date: date.to_string(),
        mood: None,
        weather: "clear".to_owned(),
        place: String::new(),
    }
}

/// Fills the diary to what one may hold: the first day by the store, and every other as
/// a copy of it, `every` days after the one before from 1900 on (a day after the other
/// is past 2009 at the limit).
async fn fill(test: &common::TestStore, own: &Store, diary_id: &str, user_id: &str, every: u32) {
    let first = day("1900-01-01");
    let template = own
        .create_diary_entry(diary_id, first, of_day(first), ENTRIES_PER_DIARY, user_id)
        .await
        .expect("made")
        .entry;
    let system = test.system();
    let more = ENTRIES_PER_DIARY - 1;
    let (numbers, day, id, fields) = match db_type() {
        DbType::Sqlite => (
            format!(
                "(WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {more}) SELECT i FROM n) AS n"
            ),
            format!("date('1900-01-01', '+' || (n.i * {every}) || ' days')"),
            "printf('e0000000-0000-4000-8000-%012x', n.i)".to_owned(),
            format!("json_object('date', date('1900-01-01', '+' || (n.i * {every}) || ' days'))"),
        ),
        DbType::Postgres => (
            format!("generate_series(1, {more}) AS n(i)"),
            format!("(DATE '1900-01-01' + n.i * {every})::text"),
            "('e0000000-0000-4000-8000-' || lpad(to_hex(n.i), 12, '0'))::uuid".to_owned(),
            format!("jsonb_build_object('date', (DATE '1900-01-01' + n.i * {every})::text)"),
        ),
    };
    let block_columns = columns_of(&system, "blocks").await;
    let block_values: Vec<String> = block_columns
        .iter()
        .map(|column| match column.as_str() {
            "id" => id.clone(),
            "title" => day.clone(),
            "fields" => fields.clone(),
            other => format!("t.{other}"),
        })
        .collect();
    let day_columns = columns_of(&system, "diary_days").await;
    let day_values: Vec<String> = day_columns
        .iter()
        .map(|column| match column.as_str() {
            "day" => day.clone(),
            "month_day" => format!("substr({day}, 6)"),
            "entry_id" => id.clone(),
            other => format!("t.{other}"),
        })
        .collect();
    for (table, columns, values, key) in [
        ("blocks", &block_columns, &block_values, "id"),
        ("diary_days", &day_columns, &day_values, "entry_id"),
    ] {
        system
            .execute_unchecked(&format!(
                "INSERT INTO {table} ({}) SELECT {} FROM {numbers}, \
                 (SELECT * FROM {table} WHERE {key} = '{}') AS t",
                columns.join(", "),
                values.join(", "),
                template.id
            ))
            .await
            .expect("the entries are copied");
    }
    if db_type() == DbType::Postgres {
        system
            .execute_unchecked("ANALYZE blocks")
            .await
            .expect("analyzed");
        system
            .execute_unchecked("ANALYZE diary_days")
            .await
            .expect("analyzed");
    }
}

/// Index of the runs query in [`Store::plans_of_diary_reads`]: only it scans a subquery
/// of its own (`written`), so changing the order in `plans_of_diary_reads` requires
/// updating this constant.
const RUNS_READ: usize = 2;

/// What the plan of a read of a diary must say, the entries of a month and day first,
/// the days of a month second and the days alone (for the runs) third: the days are
/// found by the index the read is for, an entry by its own key and not by going through
/// those of its diary, the days alone from the key without the rows, and no table is
/// read through (what the runs are put together from, SQLite goes through as a
/// subquery of its own, `written`: that is no table).
fn check_plan(read: usize, plan: &str) {
    // Only the runs (the third read) go through a subquery of their own.
    let own = |line: &str| {
        read == RUNS_READ && (line.contains("SCAN written") || line.contains("SCAN (subquery-"))
    };
    let through = |line: &str| match db_type() {
        DbType::Sqlite => line.contains("SCAN ") && !own(line),
        DbType::Postgres => line.contains("Seq Scan"),
    };
    assert!(
        !plan.lines().any(through),
        "a table is read through: {plan}"
    );
    let said: [&[&str]; 3] = match db_type() {
        DbType::Sqlite => [
            &[
                "diary_days_month_day_idx (container_id=? AND month_day=?)",
                "SEARCH blocks USING INDEX",
                "AND id=?)",
            ],
            &[
                "SEARCH diary_days USING INDEX",
                "(container_id=? AND day>? AND day<?)",
            ],
            &["SEARCH diary_days USING COVERING INDEX", "(container_id=?)"],
        ],
        DbType::Postgres => [
            &["diary_days_month_day_idx", "blocks_pkey"],
            &["diary_days_pkey", "(day >= "],
            &["Index Only Scan using diary_days_pkey"],
        ],
    };
    for part in said[read] {
        assert!(
            plan.contains(part),
            "the plan does not say `{part}`: {plan}"
        );
    }
}

/// A diary as full as one may be answers for a day and for a month from the rows of its
/// days, and reads no more of the entries than it answers with: the plans of the two
/// queries name the indexes and scan no table, and both are printed with what the reads
/// took.
#[tokio::test]
async fn a_full_diary_answers_for_a_day_without_being_read() {
    const RUNS: u32 = 20;
    let test = common::setup().await;
    let user_id = common::user(&test, "writer").await;
    let (own, workspace_id) = common::in_own_space(&test, &user_id).await;
    let diary_id = a_diary(&own, &workspace_id, &user_id).await;
    fill(&test, &own, &diary_id, &user_id, 1).await;
    // Another diary beside it, to be left out of every answer.
    let beside = a_diary(&own, &workspace_id, &user_id).await;
    let of_beside = day("2000-11-21");
    own.create_diary_entry(
        &beside,
        of_beside,
        of_day(of_beside),
        ENTRIES_PER_DIARY,
        &user_id,
    )
    .await
    .expect("made");
    // The diary is full: one more day is refused.
    let over = day("2026-11-21");
    let one_more = own
        .create_diary_entry(&diary_id, over, of_day(over), ENTRIES_PER_DIARY, &user_id)
        .await;
    assert!(one_more.is_err(), "a full diary takes a day: {one_more:?}");

    let at: MonthDay = "11-21".parse().expect("a month and day");
    let (from, to) = (day("2000-11-01"), day("2000-11-30"));
    let mut took = [Duration::ZERO; 3];
    for _ in 0..RUNS {
        let started = Instant::now();
        let entries = own.get_diary_entries_on(&diary_id, at).await.expect("read");
        took[0] += started.elapsed();
        // 1900 to 2009: a year an entry.
        assert_eq!(entries.len(), 109);
        assert!(entries.iter().all(|entry| entry.title.ends_with("-11-21")));
        assert_eq!(entries[0].title, "2008-11-21");
        let started = Instant::now();
        let days = own
            .get_diary_days(&diary_id, Some(from), Some(to))
            .await
            .expect("read");
        took[1] += started.elapsed();
        assert_eq!(days.len(), 30);
        let started = Instant::now();
        let runs = own.get_diary_runs(&diary_id).await.expect("read");
        took[2] += started.elapsed();
        assert_eq!(runs.len(), 1, "a day after the other is one run");
        assert_eq!(
            (runs[0].from, runs[0].days),
            (day("1900-01-01"), ENTRIES_PER_DIARY)
        );
    }
    let plans = own
        .plans_of_diary_reads(&diary_id, at, from, to)
        .await
        .expect("explained");
    let all = own
        .get_diary_days(&diary_id, None, None)
        .await
        .expect("read");
    assert_eq!(
        all.len(),
        usize::try_from(ENTRIES_PER_DIARY).expect("a count")
    );
    for (read, ((name, plan), took)) in [
        "the entries of 11-21 in every year",
        "the days of a month",
        "the runs of the written days",
    ]
    .into_iter()
    .zip(&plans)
    .zip(took)
    .enumerate()
    {
        println!(
            "{name}, of {ENTRIES_PER_DIARY} entries on {:?}: {:?} a read; the plan:\n  {}",
            db_type(),
            took / RUNS,
            plan.join("\n  ")
        );
        check_plan(read, &plan.join("\n"));
    }
    test.teardown().await;
}

/// The most runs a diary may have, a run a day: every other day written, to the limit.
/// They are read from the key of the days alone, and what the read took and how large
/// their answer is are printed.
#[tokio::test]
async fn a_full_diary_of_every_other_day_answers_its_runs_from_the_key() {
    const RUNS: u32 = 5;
    let test = common::setup().await;
    let user_id = common::user(&test, "writer").await;
    let (own, workspace_id) = common::in_own_space(&test, &user_id).await;
    let diary_id = a_diary(&own, &workspace_id, &user_id).await;
    fill(&test, &own, &diary_id, &user_id, 2).await;
    let mut took = Duration::ZERO;
    let mut runs = Vec::new();
    for _ in 0..RUNS {
        let started = Instant::now();
        runs = own.get_diary_runs(&diary_id).await.expect("read");
        took += started.elapsed();
    }
    assert_eq!(
        runs.len(),
        usize::try_from(ENTRIES_PER_DIARY).expect("a count")
    );
    assert!(runs.iter().all(|run| run.days == 1 && run.from == run.to));
    assert_eq!(runs[1].from, day("1900-01-03"));
    let at: MonthDay = "11-21".parse().expect("a month and day");
    let [.., plan] = own
        .plans_of_diary_reads(&diary_id, at, day("2000-11-01"), day("2000-11-30"))
        .await
        .expect("explained");
    check_plan(RUNS_READ, &plan.join("\n"));
    let answer = serde_json::to_vec(&runs).expect("serialized").len();
    println!(
        "the runs of {ENTRIES_PER_DIARY} entries a day apart on {:?}: {:?} a read, \
         {answer} bytes of JSON; the plan:\n  {}",
        db_type(),
        took / RUNS,
        plan.join("\n  ")
    );
    test.teardown().await;
}
