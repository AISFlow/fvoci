//! Diaries: where one may be and how many, the one entry of a day whoever makes it and
//! whatever brings one back, the day an entry keeps, and the days of a diary as they
//! are asked for.

use std::sync::Arc;

use fvoci_model::{
    Block, BlockError, BlockPatch, BlockType, Container, ContainerError, ContainerKind,
    ContainerVisibility, DIARIES_PER_SPACE, DiaryDate, DiaryMaking, ENTRIES_PER_DIARY, MonthDay,
    Mood, NewDiaryEntry, PublicLinkChange, new_id,
};
use fvoci_store::{DbType, Store, StoreError, Tenant};
use serde_json::json;
use tokio::sync::Barrier;
use tokio::task::JoinSet;

use crate::boards::{new_board, object};
use crate::common::{self, TestStore};

/// A diary to make in the workspace.
pub(crate) fn new_diary(workspace_id: &str) -> Container {
    Container {
        kind: ContainerKind::Diary,
        ..new_board(&new_id(), workspace_id, ContainerVisibility::Private)
    }
}

/// A user, a handle that reaches their own space, the workspace of it, and a diary there.
async fn writer(test: &TestStore, username: &str) -> (String, Store, String, String) {
    let user_id = common::user(test, username).await;
    let (own, workspace_id) = common::in_own_space(test, &user_id).await;
    let made = own
        .create_diary(new_diary(&workspace_id), &user_id, DiaryMaking::Another)
        .await
        .expect("a diary is made in the space");
    assert!(made.made.is_some(), "the diary is made: {made:?}");
    (user_id, own, workspace_id, made.diary.id)
}

fn day(text: &str) -> DiaryDate {
    text.parse().expect("a day")
}

/// What a request for the entry of a day says, with nothing but the day.
fn of_day(date: &str) -> NewDiaryEntry {
    NewDiaryEntry {
        date: date.to_owned(),
        mood: None,
        weather: String::new(),
        place: String::new(),
    }
}

/// Makes the entry of the day, which the diary did not have, and returns it.
pub(crate) async fn written(own: &Store, diary_id: &str, date: &str, user_id: &str) -> Block {
    let made = own
        .create_diary_entry(
            diary_id,
            day(date),
            of_day(date),
            ENTRIES_PER_DIARY,
            user_id,
        )
        .await
        .expect("the entry is made");
    assert!(made.made, "{date} had no entry");
    made.entry
}

fn block_refusal(result: Result<impl std::fmt::Debug, StoreError>) -> BlockError {
    match result {
        Err(StoreError::InvalidBlock { source, .. } | StoreError::NotRestorable { source, .. }) => {
            source
        }
        other => panic!("the block is refused: {other:?}"),
    }
}

fn container_refusal(result: Result<impl std::fmt::Debug, StoreError>) -> ContainerError {
    match result {
        Err(StoreError::InvalidContainer(refused)) => refused,
        other => panic!("the container is refused: {other:?}"),
    }
}

/// The rows of the days of a diary, as `day|entry|mood`, past every wall.
async fn day_rows(test: &TestStore, diary_id: &str) -> Vec<String> {
    let query = format!(
        "SELECT day || '|' || CAST(entry_id AS TEXT) || '|' || COALESCE(mood, '') \
         FROM diary_days WHERE container_id = '{diary_id}' ORDER BY day"
    );
    test.system().texts_unchecked(&query).await.expect("read")
}

/// Of the requests that make the entry of one day at once, one makes it and every other
/// is answered with that entry: the diary has one entry of the day, and one row of it.
#[tokio::test]
async fn a_day_has_one_entry_whoever_makes_it_at_once() {
    const AT_ONCE: usize = 8;
    let test = common::setup().await;
    let (user_id, own, _, diary_id) = writer(&test, "writer").await;

    let barrier = Arc::new(Barrier::new(AT_ONCE));
    let mut requests = JoinSet::new();
    for index in 0..AT_ONCE {
        let (own, diary_id, user_id) = (own.clone(), diary_id.clone(), user_id.clone());
        let barrier = Arc::clone(&barrier);
        requests.spawn(async move {
            barrier.wait().await;
            let new = NewDiaryEntry {
                weather: format!("as tab {index} saw it"),
                ..of_day("2026-11-21")
            };
            own.create_diary_entry(
                &diary_id,
                day("2026-11-21"),
                new,
                ENTRIES_PER_DIARY,
                &user_id,
            )
            .await
            .expect("every request is answered with the entry of the day")
        });
    }
    let answers = requests.join_all().await;
    let made: Vec<_> = answers.iter().filter(|answer| answer.made).collect();
    assert_eq!(made.len(), 1, "one request made the entry");
    for answer in &answers {
        assert_eq!(answer.entry, made[0].entry, "the others got that entry");
    }
    let entries = own
        .get_diary_entries_on(&diary_id, "11-21".parse().expect("a month and day"))
        .await
        .expect("read");
    assert_eq!(entries, vec![made[0].entry.clone()]);
    assert_eq!(
        day_rows(&test, &diary_id).await,
        vec![format!("2026-11-21|{}|", made[0].entry.id)]
    );
    test.teardown().await;
}

/// An entry in the trash holds no day: the day takes a new entry. Then the old one does
/// not come back, by the diary's own way or by the way of any block, and says why; once
/// the day is free again it does.
#[tokio::test]
async fn an_entry_comes_back_from_the_trash_only_to_a_free_day() {
    let test = common::setup().await;
    let (user_id, own, _, diary_id) = writer(&test, "writer").await;
    let first = written(&own, &diary_id, "2024-02-29", &user_id).await;
    own.delete_block(&first.id, &user_id)
        .await
        .expect("deleted");
    assert_eq!(day_rows(&test, &diary_id).await, Vec::<String>::new());
    let trashed = own
        .get_deleted_diary_entries(&diary_id, None)
        .await
        .expect("read");
    assert_eq!(trashed.len(), 1);
    assert_eq!(
        (trashed[0].id.as_str(), trashed[0].date.as_str()),
        (first.id.as_str(), "2024-02-29")
    );

    let second = written(&own, &diary_id, "2024-02-29", &user_id).await;
    let taken = BlockError::DayTaken {
        date: "2024-02-29".to_owned(),
    };
    let refused = own
        .undelete_diary_entry(&diary_id, &first.id, ENTRIES_PER_DIARY, &user_id)
        .await;
    assert!(
        matches!(&refused, Err(StoreError::NotRestorable { id, source }) if *id == first.id && *source == taken),
        "{refused:?}"
    );
    assert_eq!(
        block_refusal(own.undelete_block(&first.id, &user_id).await),
        taken
    );
    assert_eq!(
        day_rows(&test, &diary_id).await,
        vec![format!("2024-02-29|{}|", second.id)],
        "the refusals left the day as it was"
    );

    own.delete_block(&second.id, &user_id)
        .await
        .expect("deleted");
    let back = own
        .undelete_diary_entry(&diary_id, &first.id, ENTRIES_PER_DIARY, &user_id)
        .await
        .expect("the day is free");
    assert_eq!(back.id, first.id);
    assert_eq!(
        day_rows(&test, &diary_id).await,
        vec![format!("2024-02-29|{}|", first.id)]
    );
    // What is not an entry of this diary's trash is not found there.
    let other = own
        .undelete_diary_entry(&new_id(), &second.id, ENTRIES_PER_DIARY, &user_id)
        .await;
    assert!(matches!(other, Err(StoreError::NotFound(_))), "{other:?}");
    test.teardown().await;
}

/// Of the requests that bring an entry back and make the entry of its day at once, the
/// day has one entry afterwards, whichever came first.
#[tokio::test]
async fn a_day_has_one_entry_when_one_comes_back_as_another_is_made() {
    let test = common::setup().await;
    let (user_id, own, _, diary_id) = writer(&test, "writer").await;
    for round in 0..4 {
        let date = format!("2025-03-{:02}", round + 1);
        let old = written(&own, &diary_id, &date, &user_id).await;
        own.delete_block(&old.id, &user_id).await.expect("deleted");
        let barrier = Arc::new(Barrier::new(2));
        let restoring = {
            let (own, diary_id, user_id, old_id) = (
                own.clone(),
                diary_id.clone(),
                user_id.clone(),
                old.id.clone(),
            );
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                own.undelete_diary_entry(&diary_id, &old_id, ENTRIES_PER_DIARY, &user_id)
                    .await
            })
        };
        let making = {
            let (own, diary_id, user_id, date) =
                (own.clone(), diary_id.clone(), user_id.clone(), date.clone());
            let barrier = Arc::clone(&barrier);
            tokio::spawn(async move {
                barrier.wait().await;
                own.create_diary_entry(
                    &diary_id,
                    day(&date),
                    of_day(&date),
                    ENTRIES_PER_DIARY,
                    &user_id,
                )
                .await
            })
        };
        let restored = restoring.await.expect("joined");
        let made = making.await.expect("joined").expect("answered");
        let rows = day_rows(&test, &diary_id).await;
        let of_the_day: Vec<&String> = rows.iter().filter(|row| row.starts_with(&date)).collect();
        assert_eq!(of_the_day.len(), 1, "{date}: {rows:?}");
        match restored {
            // The old entry came back first: the request to make one was answered with it.
            Ok(back) => {
                assert_eq!((back.id.as_str(), made.made), (old.id.as_str(), false));
                assert_eq!(made.entry.id, old.id);
            }
            Err(refused) => {
                assert!(made.made, "{date}: {refused:?}");
                assert!(
                    matches!(
                        &refused,
                        StoreError::NotRestorable {
                            source: BlockError::DayTaken { .. },
                            ..
                        }
                    ),
                    "{date}: {refused:?}"
                );
            }
        }
    }
    test.teardown().await;
}

/// The day of an entry is the one it was made of: no write of the block gives it
/// another, by its field or by its title. Its mood is written, and the row of its day
/// follows.
#[tokio::test]
async fn an_entry_keeps_its_day_and_its_row_follows_its_mood() {
    let test = common::setup().await;
    let (user_id, own, _, diary_id) = writer(&test, "writer").await;
    let entry = written(&own, &diary_id, "2026-11-21", &user_id).await;

    let another_day = BlockPatch {
        title: Some("2026-11-22".to_owned()),
        updated_fields: object(json!({"date": "2026-11-22"})),
        ..BlockPatch::default()
    };
    assert_eq!(
        block_refusal(own.patch_block(&entry.id, &another_day, &user_id).await),
        BlockError::EntryDateChanges
    );
    let mut stored_anew = entry.clone();
    stored_anew.title = "2026-11-20".to_owned();
    stored_anew
        .fields
        .insert("date".to_owned(), json!("2026-11-20"));
    assert_eq!(
        block_refusal(own.insert_block(&mut stored_anew, &user_id).await),
        BlockError::EntryDateChanges
    );
    let without_a_day = BlockPatch {
        deleted_fields: vec!["date".to_owned()],
        ..BlockPatch::default()
    };
    assert!(
        own.patch_block(&entry.id, &without_a_day, &user_id)
            .await
            .is_err()
    );
    assert_eq!(
        day_rows(&test, &diary_id).await,
        vec![format!("2026-11-21|{}|", entry.id)]
    );

    for (mood, row) in [
        (Some(Mood::Great), "great"),
        (Some(Mood::Awful), "awful"),
        (None, ""),
    ] {
        let patch = match mood {
            Some(mood) => BlockPatch {
                updated_fields: object(json!({"mood": mood.as_str()})),
                ..BlockPatch::default()
            },
            None => BlockPatch {
                deleted_fields: vec!["mood".to_owned()],
                ..BlockPatch::default()
            },
        };
        own.patch_block(&entry.id, &patch, &user_id)
            .await
            .expect("the mood is written");
        assert_eq!(
            day_rows(&test, &diary_id).await,
            vec![format!("2026-11-21|{}|{row}", entry.id)]
        );
        let days = own
            .get_diary_days(&diary_id, None, None)
            .await
            .expect("read");
        assert_eq!(days.len(), 1);
        assert_eq!((days[0].date, days[0].mood), (day("2026-11-21"), mood));
    }
    test.teardown().await;
}

/// A diary holds entries, at its top, and nothing else; no board and no wiki holds an
/// entry.
#[tokio::test]
async fn a_diary_holds_entries_and_nothing_else_does() {
    let test = common::setup().await;
    let (user_id, own, workspace_id, diary_id) = writer(&test, "writer").await;
    let entry = written(&own, &diary_id, "2026-11-21", &user_id).await;
    let block = |container_id: &str, block_type: BlockType, parent_id: &str| Block {
        id: new_id(),
        container_id: container_id.to_owned(),
        parent_id: parent_id.to_owned(),
        block_type,
        ..Block::default()
    };
    for (block_type, parent_id) in [
        (BlockType::Card, ""),
        (BlockType::View, ""),
        (BlockType::Page, ""),
        (BlockType::Comment, entry.id.as_str()),
        (BlockType::Attachment, entry.id.as_str()),
    ] {
        let mut other = block(&diary_id, block_type, parent_id);
        assert_eq!(
            block_refusal(own.insert_block(&mut other, &user_id).await),
            BlockError::NotEntryOnDiary,
            "{block_type:?}"
        );
    }
    let entry_of = |container_id: &str, date: &str, parent_id: &str| Block {
        title: date.to_owned(),
        fields: object(json!({"date": date})),
        ..block(container_id, BlockType::Entry, parent_id)
    };
    assert_eq!(
        block_refusal(
            own.insert_block(&mut entry_of(&diary_id, "2026-11-22", &entry.id), &user_id)
                .await
        ),
        BlockError::EntryBelowBlock
    );
    for kind in [ContainerKind::Board, ContainerKind::Wiki] {
        let mut container = Container {
            kind,
            ..new_board(&new_id(), &workspace_id, ContainerVisibility::Private)
        };
        own.insert_container_with_admin(&mut container, &user_id)
            .await
            .expect("stored");
        assert_eq!(
            block_refusal(
                own.insert_block(&mut entry_of(&container.id, "2026-11-22", ""), &user_id)
                    .await
            ),
            BlockError::EntryOffDiary,
            "{kind:?}"
        );
    }
    // By the way of any block a second entry of a day is refused too.
    assert_eq!(
        block_refusal(
            own.insert_block(&mut entry_of(&diary_id, "2026-11-21", ""), &user_id)
                .await
        ),
        BlockError::DayTaken {
            date: "2026-11-21".to_owned()
        }
    );
    assert_eq!(day_rows(&test, &diary_id).await.len(), 1);
    test.teardown().await;
}

/// A diary is of an account's own space: the store refuses one in a workspace of an
/// organization of people and says why, and what writes the row past the store is
/// refused by the database.
#[tokio::test]
async fn a_diary_is_of_an_own_space() {
    let test = common::setup().await;
    let (user_id, own, own_workspace, diary_id) = writer(&test, "writer").await;
    common::works_in(&test, &test.workspace_id, &user_id).await;

    for making in [DiaryMaking::Another, DiaryMaking::IfNone] {
        assert_eq!(
            container_refusal(
                test.create_diary(new_diary(&test.workspace_id), &user_id, making)
                    .await
            ),
            ContainerError::DiaryOffOwnSpace
        );
    }
    assert_eq!(
        container_refusal(
            test.insert_container_with_admin(&mut new_diary(&test.workspace_id), &user_id)
                .await
        ),
        ContainerError::DiaryOffOwnSpace
    );
    let open = Container {
        visibility: ContainerVisibility::Open,
        ..new_diary(&own_workspace)
    };
    assert_eq!(
        container_refusal(own.create_diary(open, &user_id, DiaryMaking::Another).await),
        ContainerError::DiaryNotPrivate
    );
    let template = Container {
        is_template: true,
        ..new_diary(&own_workspace)
    };
    assert_eq!(
        container_refusal(
            own.create_diary(template, &user_id, DiaryMaking::Another)
                .await
        ),
        ContainerError::DiaryIsTemplate
    );

    // Past the store: a board of the organization made a diary, and a diary moved there.
    let mut board = new_board(&new_id(), &test.workspace_id, ContainerVisibility::Private);
    test.insert_container_with_admin(&mut board, &user_id)
        .await
        .expect("stored");
    let system = test.system();
    let turned = system
        .execute_unchecked(&format!(
            "UPDATE containers SET kind = 'diary' WHERE id = '{}'",
            board.id
        ))
        .await;
    assert!(
        turned.is_err(),
        "a board of an organization is made a diary"
    );
    let moved = system
        .execute_unchecked(&format!(
            "UPDATE containers SET workspace_id = '{}' WHERE id = '{diary_id}'",
            test.workspace_id
        ))
        .await;
    assert!(moved.is_err(), "a diary is moved to an organization");
    assert_eq!(
        system
            .texts_unchecked(
                "SELECT CAST(workspace_id AS TEXT) FROM containers WHERE kind = 'diary'"
            )
            .await
            .expect("read"),
        vec![own_workspace.clone()]
    );
    test.teardown().await;
}

/// A diary has no link: the store answers for it as for a board that is not there, and
/// a row written past the store is refused by the database.
#[tokio::test]
async fn a_diary_has_no_link() {
    let test = common::setup().await;
    let (user_id, own, _, diary_id) = writer(&test, "writer").await;
    let system = test.system();
    let shared = own
        .set_public_link(&diary_id, PublicLinkChange::On, "token", &user_id)
        .await;
    assert!(matches!(shared, Err(StoreError::NotFound(_))), "{shared:?}");
    let org = if common::db_type() == DbType::Postgres {
        (
            "org_id, ",
            "(SELECT org_id FROM containers WHERE kind = 'diary'), ",
        )
    } else {
        ("", "")
    };
    let linked = system
        .execute_unchecked(&format!(
            "INSERT INTO public_links ({}container_id, enabled, token, updated_at) \
             VALUES ({}'{diary_id}', true, 'token', 1)",
            org.0, org.1
        ))
        .await;
    assert!(linked.is_err(), "a diary is given a link past the store");
    assert_eq!(
        system
            .texts_unchecked("SELECT token FROM public_links")
            .await
            .expect("read"),
        Vec::<String>::new()
    );
    test.teardown().await;
}

/// A space has so many diaries and no more, counting those that come back from the
/// trash; a diary in the trash is not counted.
#[tokio::test]
async fn a_space_has_so_many_diaries() {
    let test = common::setup().await;
    let (user_id, own, workspace_id, first) = writer(&test, "writer").await;
    for _ in 1..DIARIES_PER_SPACE {
        own.create_diary(new_diary(&workspace_id), &user_id, DiaryMaking::Another)
            .await
            .expect("one more diary");
    }
    let full = ContainerError::TooManyDiaries {
        limit: DIARIES_PER_SPACE,
    };
    assert_eq!(
        container_refusal(
            own.create_diary(new_diary(&workspace_id), &user_id, DiaryMaking::Another)
                .await
        ),
        full
    );
    assert_eq!(
        container_refusal(
            own.insert_container_with_admin(&mut new_diary(&workspace_id), &user_id)
                .await
        ),
        full
    );
    // Boards and wikis are not counted, and are made as ever.
    let mut board = new_board(&new_id(), &workspace_id, ContainerVisibility::Private);
    own.insert_container_with_admin(&mut board, &user_id)
        .await
        .expect("a board beside the diaries");

    own.delete_container(&first, &user_id)
        .await
        .expect("deleted");
    own.create_diary(new_diary(&workspace_id), &user_id, DiaryMaking::Another)
        .await
        .expect("the place of the diary in the trash is free");
    assert_eq!(
        container_refusal(own.undelete_container(&first, &user_id).await),
        full
    );
    // Another account's space has its own count.
    let (_, _, _, _) = writer(&test, "another").await;
    test.teardown().await;
}

/// Who opens the diary of a space that may have none yet asks for one if there is none:
/// the first is made, every later request is answered with the oldest, and of the
/// requests made at once one makes it.
#[tokio::test]
async fn the_first_diary_of_a_space_is_made_once() {
    const AT_ONCE: usize = 6;
    let test = common::setup().await;
    let user_id = common::user(&test, "writer").await;
    let (own, workspace_id) = common::in_own_space(&test, &user_id).await;

    let barrier = Arc::new(Barrier::new(AT_ONCE));
    let mut requests = JoinSet::new();
    for _ in 0..AT_ONCE {
        let (own, workspace_id, user_id) = (own.clone(), workspace_id.clone(), user_id.clone());
        let barrier = Arc::clone(&barrier);
        requests.spawn(async move {
            barrier.wait().await;
            own.create_diary(new_diary(&workspace_id), &user_id, DiaryMaking::IfNone)
                .await
                .expect("answered with the diary of the space")
        });
    }
    let answers = requests.join_all().await;
    assert_eq!(
        answers
            .iter()
            .filter(|answer| answer.made.is_some())
            .count(),
        1
    );
    for answer in &answers {
        assert_eq!(answer.diary.id, answers[0].diary.id);
        assert_eq!(answer.diary.kind, ContainerKind::Diary);
    }
    let another = own
        .create_diary(new_diary(&workspace_id), &user_id, DiaryMaking::Another)
        .await
        .expect("a second diary");
    assert!(another.made.is_some());
    let again = own
        .create_diary(new_diary(&workspace_id), &user_id, DiaryMaking::IfNone)
        .await
        .expect("answered");
    assert_eq!(
        (again.diary.id.as_str(), again.made),
        (answers[0].diary.id.as_str(), None)
    );
    test.teardown().await;
}

/// A diary has so many entries and no more: a new one is refused, and one that would
/// come back from the trash. An entry that is there already is answered as ever.
#[tokio::test]
async fn a_diary_has_so_many_entries() {
    const LIMIT: u32 = 3;
    let test = common::setup().await;
    let (user_id, own, _, diary_id) = writer(&test, "writer").await;
    let make = async |date: &str| {
        own.create_diary_entry(&diary_id, day(date), of_day(date), LIMIT, &user_id)
            .await
    };
    let first = make("2026-01-01").await.expect("made").entry;
    own.delete_block(&first.id, &user_id)
        .await
        .expect("deleted");
    for date in ["2026-01-02", "2026-01-03", "2026-01-04"] {
        assert!(make(date).await.expect("made").made);
    }
    let full = BlockError::TooManyEntries { limit: LIMIT };
    assert_eq!(block_refusal(make("2026-01-05").await), full);
    assert_eq!(
        block_refusal(
            own.undelete_diary_entry(&diary_id, &first.id, LIMIT, &user_id)
                .await
        ),
        full
    );
    assert!(!make("2026-01-03").await.expect("the entry of the day").made);
    assert_eq!(day_rows(&test, &diary_id).await.len(), 3);
    test.teardown().await;
}

/// The entries of a month and day in every year, the latest first, and the days that
/// are written: each of one diary, whatever another diary of the same space or another
/// account's has on the same days. The 29th of February is a day of leap years alone.
#[tokio::test]
async fn the_days_of_a_diary_are_asked_for_by_month_and_day_and_by_range() {
    let test = common::setup().await;
    let (user_id, own, workspace_id, diary_id) = writer(&test, "writer").await;
    let second = own
        .create_diary(new_diary(&workspace_id), &user_id, DiaryMaking::Another)
        .await
        .expect("a second diary")
        .diary
        .id;
    let (other_id, other_own, _, other_diary) = writer(&test, "another").await;

    let mut ids = std::collections::BTreeMap::new();
    for date in [
        "2020-02-29",
        "2023-02-28",
        "2023-03-01",
        "2024-02-29",
        "2024-11-21",
        "2025-11-21",
        "2026-11-20",
        "2026-11-21",
        "2026-11-22",
    ] {
        ids.insert(date, written(&own, &diary_id, date, &user_id).await.id);
    }
    let elsewhere = written(&own, &second, "2026-11-21", &user_id).await;
    written(&other_own, &other_diary, "2026-11-21", &other_id).await;
    let mood = BlockPatch {
        updated_fields: object(json!({"mood": "good"})),
        ..BlockPatch::default()
    };
    own.patch_block(&ids["2025-11-21"], &mood, &user_id)
        .await
        .expect("written");

    let on = async |store: &Store, diary: &str, at: &str| -> Vec<String> {
        let at: MonthDay = at.parse().expect("a month and day");
        let entries = store.get_diary_entries_on(diary, at).await.expect("read");
        entries.into_iter().map(|entry| entry.title).collect()
    };
    assert_eq!(
        on(&own, &diary_id, "11-21").await,
        ["2026-11-21", "2025-11-21", "2024-11-21"]
    );
    assert_eq!(
        on(&own, &diary_id, "02-29").await,
        ["2024-02-29", "2020-02-29"]
    );
    assert_eq!(on(&own, &diary_id, "02-28").await, ["2023-02-28"]);
    assert_eq!(on(&own, &diary_id, "07-04").await, Vec::<String>::new());
    assert_eq!(on(&own, &second, "11-21").await, ["2026-11-21"]);
    // Behind the wall on PostgreSQL; on SQLite the server finds no diary of another's.
    if common::db_type() == DbType::Postgres {
        assert_eq!(
            on(&other_own, &diary_id, "11-21").await,
            Vec::<String>::new()
        );
        assert_eq!(on(&test, &diary_id, "11-21").await, Vec::<String>::new());
        assert_eq!(
            other_own
                .get_diary_days(&diary_id, None, None)
                .await
                .expect("read"),
            vec![]
        );
    }

    let days = async |from: Option<&str>, to: Option<&str>| -> Vec<String> {
        own.get_diary_days(&diary_id, from.map(day), to.map(day))
            .await
            .expect("read")
            .into_iter()
            .map(|written| {
                assert_eq!(ids[written.date.to_string().as_str()], written.entry_id);
                let mood = written.mood.map_or("", Mood::as_str);
                format!("{}{mood}", written.date)
            })
            .collect()
    };
    assert_eq!(
        days(Some("2026-11-01"), Some("2026-11-30")).await,
        ["2026-11-20", "2026-11-21", "2026-11-22"]
    );
    assert_eq!(
        days(Some("2024-02-29"), Some("2025-11-21")).await,
        ["2024-02-29", "2024-11-21", "2025-11-21good"]
    );
    assert_eq!(days(None, Some("2020-12-31")).await, ["2020-02-29"]);
    assert_eq!(days(Some("2026-11-22"), None).await, ["2026-11-22"]);
    assert_eq!(days(None, None).await.len(), ids.len());
    let of_second = own.get_diary_days(&second, None, None).await.expect("read");
    assert_eq!(of_second.len(), 1);
    assert_eq!(of_second[0].entry_id, elsewhere.id);
    test.teardown().await;
}

/// A diary goes to the trash with its entries and comes back with them, each on its
/// day; while it is there it has no day.
#[tokio::test]
async fn a_diary_comes_back_from_the_trash_with_its_days() {
    let test = common::setup().await;
    let (user_id, own, _, diary_id) = writer(&test, "writer").await;
    let kept = written(&own, &diary_id, "2026-11-21", &user_id).await;
    let trashed = written(&own, &diary_id, "2026-11-22", &user_id).await;
    own.delete_block(&trashed.id, &user_id)
        .await
        .expect("deleted");
    let before = day_rows(&test, &diary_id).await;
    assert_eq!(before, vec![format!("2026-11-21|{}|", kept.id)]);

    own.delete_container(&diary_id, &user_id)
        .await
        .expect("deleted");
    assert_eq!(day_rows(&test, &diary_id).await, Vec::<String>::new());
    let back = own
        .undelete_container(&diary_id, &user_id)
        .await
        .expect("the diary comes back");
    assert_eq!(back.kind, ContainerKind::Diary);
    assert_eq!(day_rows(&test, &diary_id).await, before);
    // The entry that was in the trash before the diary went is still there.
    let in_trash = own
        .get_deleted_diary_entries(&diary_id, None)
        .await
        .expect("read");
    assert_eq!(in_trash.len(), 1);
    assert_eq!(in_trash[0].id, trashed.id);
    test.teardown().await;
}

/// The days of a diary are behind the wall of its space: the handle of each space
/// reaches its own, that of an organization of people and one that names no
/// organization reach none, and a diary named through the handle of another space is
/// not there. Asked where there is a wall.
#[tokio::test]
async fn the_days_of_a_diary_stay_in_its_space() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let (one, ones, _, ones_diary) = writer(&test, "one").await;
    let (other, others, _, others_diary) = writer(&test, "other").await;
    written(&ones, &ones_diary, "2024-02-29", &one).await;
    for date in ["2024-02-29", "2024-03-01"] {
        written(&others, &others_diary, date, &other).await;
    }
    let count = async |store: &Store| store.count_rows("diary_days").await.expect("counted");
    assert_eq!((count(&ones).await, count(&others).await), (1, 2));
    assert_eq!(count(&test.store).await, 0, "an organization of people");
    let unset = test.scoped(Tenant::Unset);
    assert_eq!(count(&unset).await, 0, "no organization named");
    assert_eq!(count(&test.system()).await, 3, "the system's reaches all");

    let at: MonthDay = "02-29".parse().expect("a month and day");
    let entries = ones.get_diary_entries_on(&others_diary, at).await;
    assert_eq!(entries.expect("read"), [], "the entries of another's");
    let days = ones.get_diary_days(&others_diary, None, None).await;
    assert_eq!(days.expect("read"), [], "the days of another's");
    let made = ones
        .create_diary_entry(
            &others_diary,
            day("2024-03-05"),
            of_day("2024-03-05"),
            ENTRIES_PER_DIARY,
            &one,
        )
        .await;
    assert!(
        matches!(&made, Err(error) if error.is_not_found()),
        "an entry in another's diary: {made:?}"
    );
    let trash = ones.get_deleted_diary_entries(&others_diary, None).await;
    assert_eq!(trash.expect("read"), [], "the trash of another's");
    assert_eq!((count(&ones).await, count(&others).await), (1, 2));
    test.teardown().await;
}

/// An entry whose projected `blocks.body` and notes are still empty, but whose
/// collaborative `body_docs` log holds unwritten updates (`projected_seq < log_end`),
/// must move to the trash rather than being purged directly so those updates are never
/// lost if flushing fails before deletion.
#[tokio::test]
async fn an_empty_entry_with_an_unwritten_body_doc_goes_to_trash_instead_of_being_purged() {
    let test = common::setup().await;
    let (user_id, own, _, diary_id) = writer(&test, "unflushed").await;
    let entry = written(&own, &diary_id, "2024-09-10", &user_id).await;

    own.create_body_doc(&entry.id, b"initial-state", b"empty-digest")
        .await
        .expect("create body doc");
    let as_read = own
        .get_body_doc(&entry.id)
        .await
        .expect("read body doc")
        .expect("body doc exists")
        .as_read();
    own.append_body_updates(
        &entry.id,
        &[(b"unflushed-edit", &user_id)],
        as_read,
        &[],
        &[user_id.as_str()],
        fvoci_store::BodyBefore::Leave,
    )
    .await
    .expect("append unwritten update");

    assert!(
        own.delete_diary_entry(&diary_id, &entry.id, &user_id)
            .await
            .expect("delete entry"),
        "entry deleted"
    );
    let trash = own
        .get_deleted_diary_entries(&diary_id, None)
        .await
        .expect("read trash");
    assert_eq!(trash.len(), 1, "unwritten body_docs entry must go to trash");
    assert_eq!(trash[0].id, entry.id);

    own.undelete_diary_entry(&diary_id, &entry.id, ENTRIES_PER_DIARY, &user_id)
        .await
        .expect("undelete entry");
    let doc = own
        .get_body_doc(&entry.id)
        .await
        .expect("read body doc after undelete")
        .expect("body doc preserved across trash and undelete");
    assert_eq!(doc.log_end, 1);
    assert_eq!(doc.projected_seq, 0);
    assert_eq!(doc.updates.len(), 1);
    assert_eq!(doc.updates[0].bytes, b"unflushed-edit");
    test.teardown().await;
}
