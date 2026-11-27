//! Diaries through the routes: a space has its first diary once and so many at most, a
//! day has one entry however it is asked for, a day is a day of the calendar that is
//! not far ahead, an entry is changed, deleted and brought back to a day that is free,
//! and a diary goes into an archive and comes out of one as a new diary, in a space
//! alone.

use fvoci_model::{
    BODY_MAX_BYTES, BODY_MAX_NODES, Block, BlockType, Container, ContainerKind,
    ContainerVisibility, ContainersAndBlocks, DIARIES_PER_SPACE, DiaryDate, new_id,
};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::common::{ApiResponse, Client, ROOT_WORKSPACE_ID, Setup, TestServer, UNKNOWN_ID};

/// A server, and the path of the workspace of its first user's own space.
pub(crate) async fn in_a_space() -> (TestServer, String) {
    let th = Setup::local_mode().start().await.init_basic().await;
    let own = crate::own_space::own_workspace(&th.client).await.id;
    (th, format!("/w/{own}"))
}

pub(crate) fn id_of(answer: &ApiResponse) -> String {
    text_of(&answer.value(), "id")
}

pub(crate) fn text_of(value: &Value, member: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("{member} in {value}"))
        .to_owned()
}

/// Makes a diary in the space and returns its path.
pub(crate) async fn a_diary(client: &Client, ws: &str) -> String {
    let made = client.post(&format!("{ws}/diaries"), &json!({})).await;
    made.check_created();
    format!("{ws}/diaries/{}", id_of(&made))
}

/// The identifier a path ends with.
pub(crate) fn last_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Makes the entry of the day, which had none, and returns it.
pub(crate) async fn written(client: &Client, diary: &str, entry: &Value) -> Value {
    let made = client.post(&format!("{diary}/entries"), entry).await;
    made.check_created();
    made.value()
}

fn body(text: &str) -> Value {
    json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]},
    ]})
}

/// Writes the body of the entry `made`, as who read it as it was made.
pub(crate) async fn write_body(client: &Client, entry: &str, made: &Value, text: &str) -> Value {
    let patch = json!({"readAt": made["updatedAt"], "body": body(text)});
    let patched = client.patch(entry, &patch).await;
    patched.check_ok();
    patched.value()
}

/// The days of the diary that are written, as `query` asks, or those of 2024 (the year
/// the tests write in) where it asks nothing: each day and its entry.
async fn days(client: &Client, diary: &str, query: &str) -> Vec<(String, String)> {
    let query = match query {
        "" => "?from=2024-01-01&to=2024-12-31",
        query => query,
    };
    let listed = client.get(&format!("{diary}/days{query}")).await;
    listed.check_ok();
    let listed = listed.value();
    let listed = listed.as_array().expect("a list");
    listed
        .iter()
        .map(|day| (text_of(day, "date"), text_of(day, "entryId")))
        .collect()
}

/// The statuses of two answers, the lower first.
fn both(one: &ApiResponse, other: &ApiResponse) -> [StatusCode; 2] {
    let mut statuses = [one.status, other.status];
    statuses.sort();
    statuses
}

/// What opens "the diary" of a space makes it once, whoever asks first, and a space
/// has so many diaries: one more is refused with the limit, until one goes to the
/// trash.
#[tokio::test]
async fn a_space_has_its_first_diary_once_and_so_many_at_most() {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let diaries = format!("{ws}/diaries");
    let first = json!({"ifNone": true});
    let (one, other) = tokio::join!(client.post(&diaries, &first), client.post(&diaries, &first));
    assert_eq!(both(&one, &other), [StatusCode::OK, StatusCode::CREATED]);
    assert_eq!(id_of(&one), id_of(&other), "one diary for both");
    let made = one.value();
    assert_eq!(
        (&made["kind"], &made["visibility"], &made["title"]),
        (&json!("diary"), &json!("private"), &json!("")),
        "{made}"
    );
    let again = client.post(&diaries, &first).await;
    again.check_ok();
    assert_eq!(id_of(&again), id_of(&one));
    // It is its owner's, as a board they made: among their roles.
    let memberships = client.get("/users/me/memberships").await.text();
    assert!(memberships.contains(&id_of(&one)), "{memberships}");

    let unknown = client.post(&diaries, &json!({"kind": "wiki"})).await;
    unknown.check_bad_request();
    for number in 1..DIARIES_PER_SPACE {
        let more = json!({"title": format!("Volume {number}")});
        client.post(&diaries, &more).await.check_created();
    }
    let listed = client.get(&diaries).await.value();
    let most = usize::try_from(DIARIES_PER_SPACE).expect("a count");
    assert_eq!(listed.as_array().map(Vec::len), Some(most));
    assert_eq!(listed[0]["id"], made["id"], "the oldest first");
    let over = client.post(&diaries, &json!({})).await;
    over.check_bad_request();
    assert_eq!(over.error_reason(), "too_many_diaries", "{}", over.text());
    assert_eq!(over.value()["limit"], json!(DIARIES_PER_SPACE));
    // One in the trash is not counted, and does not come back to a full space.
    let trashed = format!("{ws}/containers/{}", id_of(&one));
    let deleted = client.delete(&trashed).await;
    deleted.check_status(StatusCode::NO_CONTENT);
    client.post(&diaries, &json!({})).await.check_created();
    let back = client.post(&format!("{trashed}/restore"), &json!({})).await;
    back.check_bad_request();
    assert_eq!(back.error_reason(), "too_many_diaries", "{}", back.text());
    th.teardown().await;
}

/// A day has one entry: who asks for the entry of a day that has one is answered with
/// it, and nothing of the request is stored.
#[tokio::test]
async fn a_day_has_one_entry_whoever_asks_for_it() {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let diary = a_diary(client, &ws).await;
    let entries = format!("{diary}/entries");
    let leap = json!({"date": "2024-02-29", "mood": "good", "weather": "맑음", "place": "집"});
    let (one, other) = tokio::join!(client.post(&entries, &leap), client.post(&entries, &leap));
    assert_eq!(both(&one, &other), [StatusCode::OK, StatusCode::CREATED]);
    assert_eq!(id_of(&one), id_of(&other), "one entry for both");
    let again = json!({"date": "2024-02-29", "mood": "awful", "weather": "비"});
    let there = client.post(&entries, &again).await;
    there.check_ok();
    let entry = there.value();
    assert_eq!(
        (&entry["mood"], &entry["weather"], &entry["place"]),
        (&json!("good"), &json!("맑음"), &json!("집")),
        "the entry as it was: {entry}"
    );
    assert_eq!(
        (&entry["diaryId"], &entry["body"], &entry["date"]),
        (&json!(last_of(&diary)), &Value::Null, &json!("2024-02-29")),
        "{entry}"
    );
    let listed = format!("{diary}/days?from=2024-02-01&to=2024-02-29");
    let listed = client.get(&listed).await;
    assert_eq!(
        listed.value(),
        json!([{"date": "2024-02-29", "entryId": id_of(&one), "mood": "good"}]),
        "a day says its entry and its mood, and nothing else"
    );
    th.teardown().await;
}

/// The day of an entry is a day of the calendar, written as one, and no more than two
/// days after the server's own; the mood, the weather and the place are what they may
/// be, and an entry is made with nothing else.
#[tokio::test]
async fn the_day_of_an_entry_is_a_day_of_the_calendar_not_far_ahead() {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let diary = a_diary(client, &ws).await;
    let entries = format!("{diary}/entries");
    for date in [
        "2026-02-30",
        "2025-02-29",
        "2026-13-01",
        "2026-00-10",
        "0000-01-01",
        "20260101",
        "2026-1-1",
        "2026-11-21T00:00:00Z",
        " 2026-11-21",
        "어제",
        "",
    ] {
        let refused = client.post(&entries, &json!({ "date": date })).await;
        refused.check_bad_request();
        assert_eq!(refused.refused_at(), "/date", "{date}: {}", refused.text());
    }
    for (member, value) in [
        ("date", json!(20_261_121)),
        ("mood", json!("ecstatic")),
        ("mood", json!(3)),
        ("weather", json!("가".repeat(101))),
        ("place", json!("p".repeat(101))),
        ("title", json!("a title of its own")),
        ("body", body("not written here")),
    ] {
        let mut entry = json!({"date": "2024-03-01"});
        entry[member] = value;
        client.post(&entries, &entry).await.check_bad_request();
    }
    let longest = json!({"date": "2024-03-01", "weather": "가".repeat(100), "mood": null});
    written(client, &diary, &longest).await;

    // The server does not know where the writer is: every time zone's today is within
    // two days of its own day.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a time after 1970");
    let now = i64::try_from(now.as_millis()).expect("milliseconds");
    let today = DiaryDate::of_utc_millis(now).expect("today");
    let ahead = |days: i32| today.plus_days(days).expect("a day").to_string();
    for days in [3, 400] {
        let refused = client.post(&entries, &json!({"date": ahead(days)})).await;
        refused.check_bad_request();
        assert_eq!(refused.refused_at(), "/date", "{}", refused.text());
    }
    for days in [-1, 0, 2] {
        written(client, &diary, &json!({"date": ahead(days)})).await;
    }
    written(client, &diary, &json!({"date": "1987-06-10"})).await;
    for (from, to, written) in [
        (ahead(-1), ahead(2), 3),
        ("1987-06-10".to_owned(), "1987-06-10".to_owned(), 1),
        ("2024-03-01".to_owned(), "2024-03-01".to_owned(), 1),
    ] {
        let listed = days(client, &diary, &format!("?from={from}&to={to}")).await;
        assert_eq!(listed.len(), written, "from {from} to {to}");
    }
    th.teardown().await;
}

/// The page of a day lists its entries in every year, the latest first, and the 29th of
/// February those of leap years; the days that are written are listed the earliest
/// first, as far as they are asked.
#[tokio::test]
async fn the_days_of_a_diary_are_read_by_month_and_day_and_by_range() {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let diary = a_diary(client, &ws).await;
    for date in ["2023-02-28", "2020-02-29", "2024-02-29", "2020-02-28"] {
        written(client, &diary, &json!({ "date": date })).await;
    }
    let other = a_diary(client, &ws).await;
    written(client, &other, &json!({"date": "2016-02-29"})).await;
    let on = async |month_day: &str| {
        let listed = client.get(&format!("{diary}/days/{month_day}")).await;
        listed.check_ok();
        let listed = listed.value();
        let listed = listed.as_array().expect("a list");
        let dates: Vec<String> = listed.iter().map(|entry| text_of(entry, "date")).collect();
        dates
    };
    assert_eq!(on("02-29").await, ["2024-02-29", "2020-02-29"]);
    assert_eq!(on("02-28").await, ["2023-02-28", "2020-02-28"]);
    assert_eq!(on("12-25").await, [""; 0]);
    for no_day in ["02-30", "2-29", "13-01", "0229", "2024-02-29", "today"] {
        let refused = client.get(&format!("{diary}/days/{no_day}")).await;
        refused.check_bad_request();
        assert_eq!(refused.refused_at(), "parameter monthDay", "{no_day}");
    }

    let dates = async |query: &str| -> Vec<String> {
        let listed = days(client, &diary, query).await;
        listed.into_iter().map(|(date, _)| date).collect()
    };
    assert_eq!(
        dates("?from=2020-01-01&to=2020-12-31").await,
        ["2020-02-28", "2020-02-29"]
    );
    assert_eq!(
        dates("?from=2024-02-01&to=2024-02-29").await,
        ["2024-02-29"]
    );
    // A year at most, both of its ends counted; a range that ends before it begins is
    // of no days.
    assert_eq!(
        dates("?from=2023-02-28&to=2024-02-28").await,
        ["2023-02-28"]
    );
    assert_eq!(dates("?from=2024-03-01&to=2024-03-31").await, [""; 0]);
    assert_eq!(dates("?from=2024-03-01&to=2020-01-01").await, [""; 0]);
    for (query, place) in [
        ("?from=2024-02-30&to=2024-03-01", "from"),
        ("?from=2024-02-01&to=march", "to"),
        ("", "from"),
        ("?to=2024-02-01", "from"),
        ("?from=2024-02-01", "to"),
        ("?from=2023-02-28&to=2024-02-29", "to"),
    ] {
        let refused = client.get(&format!("{diary}/days{query}")).await;
        refused.check_bad_request();
        assert_eq!(
            refused.refused_at(),
            format!("parameter {place}"),
            "{query}"
        );
    }
    let too_wide = format!("{diary}/days?from=2023-02-28&to=2024-02-29");
    let too_wide = client.get(&too_wide).await.value();
    assert_eq!(too_wide["limit"], json!(fvoci_model::DAYS_PER_LIST));
    th.teardown().await;
}

/// The runs of a diary are its written days one after the other, across the ends of
/// months and years and through 29 February, of that diary alone; an entry in the trash
/// breaks its run, and one brought back joins it again.
#[tokio::test]
async fn the_runs_of_a_diary_are_its_written_days_one_after_the_other() {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let diary = a_diary(client, &ws).await;
    let runs = async || -> Vec<(String, String, u64)> {
        let listed = client.get(&format!("{diary}/runs")).await;
        listed.check_ok();
        let listed = listed.value();
        let listed = listed.as_array().expect("a list");
        listed
            .iter()
            .map(|run| {
                let days = run["days"].as_u64().expect("a count");
                (text_of(run, "from"), text_of(run, "to"), days)
            })
            .collect()
    };
    let run = |from: &str, to: &str, days: u64| (from.to_owned(), to.to_owned(), days);
    assert_eq!(runs().await, []);
    let mut made = Vec::new();
    for date in [
        "2024-03-03",
        "2023-12-31",
        "2024-02-28",
        "2024-01-01",
        "2024-02-29",
        "2024-03-01",
        "2025-02-28",
        "2025-03-01",
    ] {
        let entry = if date == "2024-02-29" {
            json!({ "date": date, "mood": "good" })
        } else {
            json!({ "date": date })
        };
        made.push(written(client, &diary, &entry).await);
    }
    let other = a_diary(client, &ws).await;
    written(client, &other, &json!({"date": "2024-03-02"})).await;
    assert_eq!(
        runs().await,
        [
            run("2023-12-31", "2024-01-01", 2),
            run("2024-02-28", "2024-03-01", 3),
            run("2024-03-03", "2024-03-03", 1),
            // 2025 has no 29 February.
            run("2025-02-28", "2025-03-01", 2),
        ]
    );
    let leap = format!("{diary}/entries/{}", text_of(&made[4], "id"));
    client
        .delete(&leap)
        .await
        .check_status(StatusCode::NO_CONTENT);
    assert_eq!(
        runs().await,
        [
            run("2023-12-31", "2024-01-01", 2),
            run("2024-02-28", "2024-02-28", 1),
            run("2024-03-01", "2024-03-01", 1),
            run("2024-03-03", "2024-03-03", 1),
            run("2025-02-28", "2025-03-01", 2),
        ]
    );
    client
        .post(&format!("{leap}/restore"), &json!({}))
        .await
        .check_ok();
    assert_eq!(runs().await[1], run("2024-02-28", "2024-03-01", 3));
    th.teardown().await;
}

/// The body and the notes of a day go in one patch under one `readAt`, as a page sends
/// them when it goes: both are written, or, where the entry changed since it was read,
/// neither.
#[tokio::test]
async fn the_body_and_the_notes_are_written_together_or_not_at_all() {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let diary = a_diary(client, &ws).await;
    let made = written(client, &diary, &json!({"date": "2024-03-01"})).await;
    let entry = format!("{diary}/entries/{}", text_of(&made, "id"));
    let together = json!({
        "readAt": made["updatedAt"], "body": body("함께"), "mood": "good", "weather": "비",
    });
    let both = client.patch(&entry, &together).await.value();
    assert_eq!(
        (&both["body"], &both["mood"], &both["weather"]),
        (&body("함께"), &json!("good"), &json!("비"))
    );
    let stale = json!({
        "readAt": made["updatedAt"], "body": body("늦게"), "mood": "awful", "place": "집",
    });
    client
        .patch(&entry, &stale)
        .await
        .check_status(StatusCode::CONFLICT);
    let read = client.get(&entry).await.value();
    assert_eq!(
        (&read["body"], &read["mood"], &read["place"]),
        (&body("함께"), &json!("good"), &json!(""))
    );
    th.teardown().await;
}

/// An entry is changed by its patch, which has no member for its day, and its body is
/// kept in versions as a card's is. The entry of another diary, and of nothing, is no
/// entry of this one.
#[tokio::test]
async fn an_entry_is_changed_by_its_patch_and_keeps_its_day() {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let diary = a_diary(client, &ws).await;
    let made = written(
        client,
        &diary,
        &json!({"date": "2024-02-29", "mood": "bad"}),
    )
    .await;
    let id = text_of(&made, "id");
    let entry = format!("{diary}/entries/{id}");

    let noted = json!({"mood": "great", "weather": "눈", "place": "속초"});
    let patched = client.patch(&entry, &noted).await;
    patched.check_ok();
    let patched = patched.value();
    assert_eq!(
        (&patched["mood"], &patched["weather"], &patched["place"]),
        (&json!("great"), &json!("눈"), &json!("속초"))
    );
    let cleared = json!({"mood": null, "place": ""});
    let cleared = client.patch(&entry, &cleared).await.value();
    assert_eq!(
        (&cleared["mood"], &cleared["weather"], &cleared["place"]),
        (&Value::Null, &json!("눈"), &json!(""))
    );
    for refused in [
        json!({"date": "2024-03-01"}),
        json!({"title": "2024-03-01"}),
        json!({"fields": {"date": "2024-03-01"}}),
        json!({"mood": "ecstatic"}),
        json!({"weather": "w".repeat(101)}),
        json!({"body": body("without having read it")}),
    ] {
        client.patch(&entry, &refused).await.check_bad_request();
    }
    let read = client.get(&entry).await.value();
    assert_eq!(read["date"], "2024-02-29", "the day is what it was");

    // The body, written whole by who read the entry as it is.
    let with_body = write_body(client, &entry, &read, "첫 줄").await;
    assert_eq!(with_body["body"], body("첫 줄"));
    let stale = json!({"readAt": read["updatedAt"], "body": body("늦은 줄")});
    let late = client.patch(&entry, &stale).await;
    late.check_status(StatusCode::CONFLICT);
    write_body(client, &entry, &with_body, "둘째 줄").await;
    let versions = client.get(&format!("{entry}/body-versions")).await;
    versions.check_ok();
    let versions = versions.value();
    let numbers = versions.as_array().expect("a list").iter();
    let oldest = numbers
        .filter_map(|version| version["version"].as_i64())
        .min()
        .unwrap_or_else(|| panic!("the body has versions: {versions}"));
    let version = format!("{entry}/body-versions/{oldest}");
    let kept = client.get(&version).await;
    kept.check_ok();
    let restored = client.post(&format!("{version}/restore"), &json!({})).await;
    restored.check_ok();
    assert_eq!(restored.value()["body"], kept.value()["body"]);
    assert_eq!(restored.value()["date"], "2024-02-29");
    let no_version = client.get(&format!("{entry}/body-versions/999")).await;
    no_version.check_not_found();

    let other = a_diary(client, &ws).await;
    for elsewhere in [
        format!("{other}/entries/{id}"),
        format!("{diary}/entries/{UNKNOWN_ID}"),
        format!("{ws}/diaries/{UNKNOWN_ID}/entries/{id}"),
    ] {
        client.get(&elsewhere).await.check_not_found();
        client.patch(&elsewhere, &noted).await.check_not_found();
        client.delete(&elsewhere).await.check_not_found();
    }
    assert_eq!(days(client, &other, "").await, []);
    th.teardown().await;
}

/// An entry in the trash holds no day: the day takes another entry, and then the first
/// does not come back until the day is free again.
#[tokio::test]
async fn an_entry_in_the_trash_holds_no_day_and_comes_back_to_a_free_one() {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let diary = a_diary(client, &ws).await;
    let made = written(
        client,
        &diary,
        &json!({"date": "2024-02-29", "mood": "bad"}),
    )
    .await;
    let id = text_of(&made, "id");
    let entry = format!("{diary}/entries/{id}");
    let with_body = write_body(client, &entry, &made, "남겨 둔 글").await;

    let deleted = client.delete(&entry).await;
    deleted.check_status(StatusCode::NO_CONTENT);
    client.get(&entry).await.check_not_found();
    assert_eq!(days(client, &diary, "").await, []);
    let trash = client.get(&format!("{diary}/trash/entries")).await;
    trash.check_ok();
    let trash = trash.value();
    assert_eq!(trash.as_array().map(Vec::len), Some(1), "{trash}");
    assert_eq!(
        (&trash[0]["id"], &trash[0]["date"], &trash[0]["mood"]),
        (&json!(id), &json!("2024-02-29"), &json!("bad")),
        "{trash}"
    );
    let anew = written(client, &diary, &json!({"date": "2024-02-29"})).await;
    assert_ne!(anew["id"], json!(id), "a new entry of the day");
    let undelete = format!("{entry}/restore");
    let taken = client.post(&undelete, &json!({})).await;
    taken.check_status(StatusCode::CONFLICT);
    assert_eq!(taken.error_reason(), "day_taken", "{}", taken.text());
    assert_eq!(taken.problem_type(), "/problems/day-taken");
    let the_new = format!("{diary}/entries/{}", text_of(&anew, "id"));
    let deleted = client.delete(&the_new).await;
    deleted.check_status(StatusCode::NO_CONTENT);
    let back = client.post(&undelete, &json!({})).await;
    back.check_ok();
    assert_eq!(back.value()["body"], with_body["body"], "with its body");
    assert_eq!(
        days(client, &diary, "").await,
        [("2024-02-29".to_owned(), id.clone())]
    );
    client.post(&undelete, &json!({})).await.check_not_found();
    let nothing = format!("{diary}/entries/{UNKNOWN_ID}/restore");
    client.post(&nothing, &json!({})).await.check_not_found();
    th.teardown().await;
}

/// A diary of the space with the entries of two days, one with a body, and an entry in
/// the trash: its path, the entry with the body, and its archive.
async fn an_archived_diary(client: &Client, ws: &str) -> (String, Value, Vec<u8>) {
    let diary = a_diary(client, ws).await;
    let first = written(
        client,
        &diary,
        &json!({"date": "2024-02-29", "mood": "good"}),
    )
    .await;
    let entry = format!("{diary}/entries/{}", text_of(&first, "id"));
    write_body(client, &entry, &first, "보름달을 봤다").await;
    written(
        client,
        &diary,
        &json!({"date": "2024-03-01", "place": "강릉"}),
    )
    .await;
    let gone = written(
        client,
        &diary,
        &json!({"date": "2024-03-02", "mood": "okay"}),
    )
    .await;
    let gone = format!("{diary}/entries/{}", text_of(&gone, "id"));
    let deleted = client.delete(&gone).await;
    deleted.check_status(StatusCode::NO_CONTENT);
    let exported = format!("{ws}/containers/{}/archive/export", last_of(&diary));
    let exported = client.get(&exported).await;
    exported.check_ok();
    (diary, first, exported.body)
}

/// A diary goes into an archive with its entries and comes out of one as a new diary of
/// the space, beside the one it was taken of.
#[tokio::test]
async fn a_diary_is_archived_and_comes_back_as_a_new_one() {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let own = last_of(&ws).to_owned();
    let (diary, first, archive) = an_archived_diary(client, &ws).await;
    crate::export::import(client, &own, &archive)
        .await
        .check_ok();
    let listed = client.get(&format!("{ws}/diaries")).await.value();
    let listed = listed.as_array().expect("a list");
    assert_eq!(listed.len(), 2, "a new diary beside the first");
    let copy = listed
        .iter()
        .find(|listed| listed["id"] != json!(last_of(&diary)))
        .expect("the new one");
    assert_eq!(copy["kind"], "diary", "{copy}");
    let copy = format!("{ws}/diaries/{}", text_of(copy, "id"));
    let (of_first, of_copy) = (
        days(client, &diary, "").await,
        days(client, &copy, "").await,
    );
    let dates = |days: &[(String, String)]| -> Vec<String> {
        days.iter().map(|(date, _)| date.clone()).collect()
    };
    assert_eq!(dates(&of_copy), ["2024-02-29", "2024-03-01"]);
    assert_eq!(dates(&of_copy), dates(&of_first));
    assert!(
        of_copy.iter().all(|day| !of_first.contains(day)),
        "the entries of the copy are its own: {of_copy:?}"
    );
    let read = client.get(&format!("{copy}/days/02-29")).await.value();
    assert_eq!(
        (&read[0]["mood"], &read[0]["body"]),
        (&json!("good"), &body("보름달을 봤다")),
        "{read}"
    );
    // What is asked of one diary is of that diary alone.
    let on_the_day = client.get(&format!("{diary}/days/02-29")).await.value();
    assert_eq!(on_the_day.as_array().map(Vec::len), Some(1));
    assert_eq!(on_the_day[0]["id"], first["id"]);
    th.teardown().await;
}

/// The files of an archive, each with its lines.
fn archive_lines(bytes: &[u8]) -> Vec<(String, Vec<Value>)> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("an archive");
    let mut files = Vec::new();
    for index in 0..zip.len() {
        let mut file = zip.by_index(index).expect("a file");
        let name = file.name().expect("its name").into_owned();
        let mut text = String::new();
        std::io::Read::read_to_string(&mut file, &mut text).expect("its text");
        let lines = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("a line of JSON"))
            .collect();
        files.push((name, lines));
    }
    files
}

fn archive_of_lines(files: &[(String, Vec<Value>)]) -> Vec<u8> {
    use std::io::Write as _;

    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, lines) in files {
        zip.start_file(name.as_str(), options).expect("a file");
        for line in lines {
            writeln!(zip, "{line}").expect("a line");
        }
    }
    zip.finish().expect("the archive").into_inner()
}

/// The archive with one more entry in each of its diaries: a copy of an entry of it
/// under a new identifier, changed by `change`, which is given the entry it copies.
fn spoiled(archive: &[u8], change: &dyn Fn(&mut Value, &Value)) -> Vec<u8> {
    let mut files = archive_lines(archive);
    for (_, lines) in &mut files {
        let Some(entry) = lines.iter().find(|line| line["data"]["type"] == "entry") else {
            continue;
        };
        let mut more = entry.clone();
        more["data"]["id"] = json!(fvoci_model::new_id());
        change(&mut more, entry);
        lines.push(more);
    }
    archive_of_lines(&files)
}

/// No workspace of an organization takes a diary from an archive, and an archive in
/// which a day has two entries, an entry is below another, a diary holds a card or a
/// day is none is taken nowhere: nothing of any of them is stored.
#[tokio::test]
async fn an_archive_puts_a_diary_in_a_space_alone_and_as_a_diary_is() {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let own = last_of(&ws).to_owned();
    let (_, _, archive) = an_archived_diary(client, &ws).await;
    let stored = async || {
        let count = "SELECT CAST(count(*) AS TEXT) FROM containers WHERE kind = 'diary'";
        // Of every organization: the space is none of the server's own.
        let counted = th.app.store().system().texts_unchecked(count).await;
        counted.expect("counted")
    };
    assert_eq!(stored().await, ["1"]);
    let refused = crate::export::import(client, ROOT_WORKSPACE_ID, &archive).await;
    refused.check_bad_request();
    let whole = client.get(&format!("{ws}/archive/export")).await;
    whole.check_ok();
    let refused = crate::export::import(client, ROOT_WORKSPACE_ID, &whole.body).await;
    refused.check_bad_request();
    assert_eq!(stored().await, ["1"], "an organization takes no diary");

    let another_day = |more: &mut Value, date: &str| {
        more["data"]["title"] = json!(date);
        more["data"]["fields"]["date"] = json!(date);
    };
    let twice = spoiled(&archive, &|_, _| {});
    let below = spoiled(&archive, &|more, entry| {
        another_day(more, "2019-01-01");
        more["data"]["parentId"] = entry["data"]["id"].clone();
    });
    let a_card = spoiled(&archive, &|more, _| more["data"]["type"] = json!("card"));
    let no_day = spoiled(&archive, &|more, _| another_day(more, "2019-02-30"));
    let to_come = spoiled(&archive, &|more, _| another_day(more, "9999-12-31"));
    for (what, spoiled) in [
        ("a day twice", twice),
        ("an entry below an entry", below),
        ("a card", a_card),
        ("a day that is none", no_day),
        ("a day yet to come", to_come),
    ] {
        let refused = crate::export::import(client, &own, &spoiled).await;
        let (status, text) = (refused.status, refused.text());
        assert_eq!(status, StatusCode::BAD_REQUEST, "{what}: {text}");
        assert_eq!(stored().await, ["1"], "{what}: nothing was stored");
    }
    // One more entry of a day of its own is taken, as the archive itself is.
    let one_more = spoiled(&archive, &|more, _| another_day(more, "2019-01-01"));
    for archive in [&one_more, &archive] {
        crate::export::import(client, &own, archive)
            .await
            .check_ok();
    }
    assert_eq!(stored().await, ["3"]);
    th.teardown().await;
}

/// An archive whose diaries the workspace does not take stores nothing, not the board
/// that comes before a diary in it either: no workspace of an organization of people
/// takes a diary, and a space takes no more diaries than it may have.
#[tokio::test]
async fn an_archive_with_diaries_a_workspace_does_not_take_stores_nothing() {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let own = last_of(&ws).to_owned();
    let board = json!({"visibility": "private", "title": "Beside the diaries"});
    let board = client.post(&format!("{ws}/boards"), &board).await;
    assert!(board.status.is_success(), "{}", board.text());
    let (_, _, one_diary) = an_archived_diary(client, &ws).await;
    a_diary(client, &ws).await;
    let whole = client.get(&format!("{ws}/archive/export")).await;
    whole.check_ok();
    // The board first, as an archive may have it: it would be stored before a diary
    // is refused.
    let of_a_diary = |lines: &Vec<Value>| lines.iter().any(|line| line["data"]["kind"] == "diary");
    let mut files = archive_lines(&whole.body);
    files.sort_by_key(|(_, lines)| of_a_diary(lines));
    let diaries = files.iter().filter(|(_, lines)| of_a_diary(lines)).count();
    let boards = files
        .iter()
        .filter(|(name, _)| name.ends_with("container.jsonl"));
    assert_eq!((boards.count(), diaries), (3, 2), "a board and two diaries");
    let whole = archive_of_lines(&files);
    let stored = async || {
        let count = "SELECT CAST(count(*) AS TEXT) FROM containers";
        let counted = th.app.store().system().texts_unchecked(count).await;
        counted.expect("counted")
    };

    let before = stored().await;
    let refused = crate::export::import(client, ROOT_WORKSPACE_ID, &whole).await;
    refused.check_bad_request();
    assert_eq!(stored().await, before, "nor the board beside the diaries");
    // One diary short of full, the space has no room for the two of the archive.
    for _ in 2..DIARIES_PER_SPACE - 1 {
        a_diary(client, &ws).await;
    }
    let before = stored().await;
    let refused = crate::export::import(client, &own, &whole).await;
    refused.check_bad_request();
    assert_eq!(
        refused.error_reason(),
        "too_many_diaries",
        "{}",
        refused.text()
    );
    assert_eq!(stored().await, before, "nor the board beside the diaries");
    crate::export::import(client, &own, &one_diary)
        .await
        .check_ok();
    let listed = client.get(&format!("{ws}/diaries")).await.value();
    let most = usize::try_from(DIARIES_PER_SPACE).expect("a count");
    assert_eq!(listed.as_array().map(Vec::len), Some(most));
    th.teardown().await;
}

/// An entry is found by what its body says, in the search of its space, as an entry of
/// its diary; and the account's deletion counts the diaries of its space.
#[tokio::test]
async fn an_entry_is_searched_and_a_diary_is_counted_in_its_space() {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let diary = a_diary(client, &ws).await;
    let made = written(client, &diary, &json!({"date": "2024-02-29"})).await;
    let entry = format!("{diary}/entries/{}", text_of(&made, "id"));
    write_body(client, &entry, &made, "보름달을 봤다").await;
    let told = th.app.finish_background(std::time::Duration::from_secs(5));
    assert!(told.await, "the background work ends");

    let found = client.get(&format!("{ws}/search?q=보름달")).await;
    found.check_ok();
    let found = found.value();
    assert_eq!(found.as_array().map(Vec::len), Some(1), "{found}");
    let hit = &found[0];
    assert_eq!(
        (&hit["id"], &hit["kind"], &hit["containerKind"]),
        (&made["id"], &json!("entry"), &json!("diary")),
        "{found}"
    );
    assert_eq!(
        (&hit["containerId"], &hit["title"]),
        (&json!(last_of(&diary)), &json!("2024-02-29")),
        "an entry is titled by its day: {found}"
    );
    for (query, hits) in [("kind=entry", 1), ("kind=card", 0), ("kind=page", 0)] {
        let found = client.get(&format!("{ws}/search?q=보름달&{query}")).await;
        found.check_ok();
        assert_eq!(
            found.value().as_array().map(Vec::len),
            Some(hits),
            "{query}"
        );
    }
    // The workspace of the organization the user works in finds nothing of it.
    let elsewhere = format!("/w/{ROOT_WORKSPACE_ID}/search?q=보름달");
    let elsewhere = client.get(&elsewhere).await;
    elsewhere.check_ok();
    assert_eq!(elsewhere.value(), json!([]));
    // No reference names an entry.
    let resolve = format!("{ws}/references/resolve");
    let named = client.post(&resolve, &json!([made["id"]])).await;
    named.check_ok();
    assert!(!named.text().contains("2024-02-29"), "{}", named.text());

    let deletion = client.get("/users/me/deletion").await;
    deletion.check_ok();
    let deletion = deletion.value();
    let organizations = deletion["organizations"].as_array().expect("a list");
    let space = organizations
        .iter()
        .find(|organization| organization["kind"] == "personal")
        .expect("the space");
    assert_eq!(
        (&space["diaries"], &space["boards"]),
        (&json!(1), &json!(0)),
        "{deletion}"
    );
    th.teardown().await;
}

/// Conditional body writes compare `readAt` against when the entry's body last changed:
/// metadata patches (`mood`, `weather`, `place`) do not cause 409 conflicts, while any
/// body write (patch or collaborative edit) or forged timestamp still returns 409.
#[tokio::test]
async fn metadata_patches_do_not_conflict_with_conditional_body_writes() {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let own = last_of(&ws).to_owned();
    let diary = a_diary(client, &ws).await;
    let made = written(client, &diary, &json!({"date": "2024-05-01"})).await;
    let entry_id = text_of(&made, "id");
    let entry = format!("{diary}/entries/{entry_id}");
    let t0 = made["updatedAt"].as_i64().expect("t0");

    // Metadata-only patches advance updatedAt without advancing the body timestamp.
    for meta in [
        json!({"mood": "good"}),
        json!({"weather": "맑음"}),
        json!({"place": "서울"}),
    ] {
        client.patch(&entry, &meta).await.check_ok();
    }
    let after_meta = client.get(&entry).await.value();
    let t_meta = after_meta["updatedAt"].as_i64().expect("t_meta");
    assert!(t_meta > t0, "metadata patches advanced updatedAt");

    // Body write using pre-metadata readAt succeeds!
    let with_body = client
        .patch(&entry, &json!({"readAt": t0, "body": body("본문 첫 저장")}))
        .await;
    with_body.check_ok();
    let with_body = with_body.value();
    let t_body = with_body["updatedAt"].as_i64().expect("t_body");
    assert_eq!(with_body["mood"], "good");
    assert_eq!(with_body["weather"], "맑음");
    assert_eq!(with_body["place"], "서울");

    // Another metadata patch does not invalidate t_body, and t_meta2 also works.
    let meta2 = client
        .patch(&entry, &json!({"mood": "great"}))
        .await
        .value();
    let t_meta2 = meta2["updatedAt"].as_i64().expect("t_meta2");
    assert!(t_meta2 > t_body);
    let body2 = client
        .patch(
            &entry,
            &json!({"readAt": t_body, "body": body("본문 둘째")}),
        )
        .await;
    body2.check_ok();
    let body2 = body2.value();
    let t_body2 = body2["updatedAt"].as_i64().expect("t_body2");

    // Stale readAt from before the body write is still 409, as is a forged future readAt.
    client
        .patch(&entry, &json!({"readAt": t_body, "body": body("stale")}))
        .await
        .check_status(StatusCode::CONFLICT);
    client
        .patch(
            &entry,
            &json!({"readAt": t_body2 + 60_000, "body": body("future")}),
        )
        .await
        .check_status(StatusCode::CONFLICT);

    // Collaborative body edit advances the body timestamp and blocks stale readAt.
    let mut peer = crate::rooms::Peer::connect(client, 201).await;
    peer.joined(&own, &entry_id).await;
    peer.write_acked("방에서 추가").await;
    client
        .patch(
            &entry,
            &json!({"readAt": t_body2, "body": body("overwrite")}),
        )
        .await
        .check_status(StatusCode::CONFLICT);
    peer.leave().await;
    th.teardown().await;
}

/// `appendBody` appends blocks atomically without `readAt`, coordinates with open
/// collaborative rooms, rejects simultaneous `body` + `appendBody`, and enforces
/// `BODY_MAX_BYTES` and `BODY_MAX_NODES` across existing + appended content.
async fn assert_append_body_limits(client: &Client, diary: &str) {
    // Combined BODY_MAX_BYTES overflow is refused at `/appendBody` without modifying notes.
    let big_entry = written(client, diary, &json!({"date": "2024-06-02"})).await;
    let big_at = format!("{diary}/entries/{}", text_of(&big_entry, "id"));
    let half_bytes = body(&"a".repeat(BODY_MAX_BYTES / 2 + 1024));
    client
        .patch(&big_at, &json!({"appendBody": half_bytes}))
        .await
        .check_ok();
    let too_big = client
        .patch(
            &big_at,
            &json!({"appendBody": half_bytes, "place": "저장되면 안 됨"}),
        )
        .await;
    too_big.check_bad_request();
    assert_eq!(too_big.refused_at(), "/appendBody", "{}", too_big.text());
    assert_eq!(
        client.get(&big_at).await.value()["place"],
        "",
        "refused appendBody must not modify notes"
    );

    // Combined BODY_MAX_NODES overflow is refused at `/appendBody`.
    let node_entry = written(client, diary, &json!({"date": "2024-06-03"})).await;
    let node_at = format!("{diary}/entries/{}", text_of(&node_entry, "id"));
    let half_nodes = json!({
        "type": "doc",
        "content": vec![json!({"type": "horizontalRule"}); BODY_MAX_NODES / 2],
    });
    client
        .patch(&node_at, &json!({"appendBody": half_nodes}))
        .await
        .check_ok();
    let too_many_nodes = client
        .patch(&node_at, &json!({"appendBody": half_nodes}))
        .await;
    too_many_nodes.check_bad_request();
    assert_eq!(
        too_many_nodes.refused_at(),
        "/appendBody",
        "{}",
        too_many_nodes.text()
    );

    // Foreign/missing file reference in appendBody reports `/appendBody` (not `/body`).
    let missing_file_body = json!({
        "type": "doc",
        "content": [{"type": "image", "attrs": {"fileId": "does-not-exist.png"}}],
    });
    let bad_file = client
        .patch(&node_at, &json!({"appendBody": missing_file_body}))
        .await;
    bad_file.check_bad_request();
    assert_eq!(bad_file.refused_at(), "/appendBody", "{}", bad_file.text());
}

#[tokio::test]
async fn append_body_appends_atomically_coordinates_with_room_and_enforces_limits() {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let own = last_of(&ws).to_owned();
    let diary = a_diary(client, &ws).await;
    let made = written(client, &diary, &json!({"date": "2024-06-01"})).await;
    let entry_id = text_of(&made, "id");
    let entry = format!("{diary}/entries/{entry_id}");

    // Append to an empty entry (no readAt required) together with notes.
    let first = client
        .patch(
            &entry,
            &json!({"appendBody": body("첫 문단"), "weather": "비"}),
        )
        .await;
    first.check_ok();
    let first = first.value();
    assert_eq!(first["body"], body("첫 문단"));
    assert_eq!(first["weather"], "비");

    // Append a second paragraph; null or empty paragraph leaves body unchanged.
    let second = client
        .patch(&entry, &json!({"appendBody": body("둘째 문단")}))
        .await;
    second.check_ok();
    assert_eq!(
        second.value()["body"],
        json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "첫 문단"}]},
            {"type": "paragraph", "content": [{"type": "text", "text": "둘째 문단"}]},
        ]})
    );
    client
        .patch(&entry, &json!({"appendBody": Value::Null, "mood": "okay"}))
        .await
        .check_ok();
    assert_eq!(
        client.get(&entry).await.value()["body"],
        second.value()["body"]
    );

    // Sending both `body` and `appendBody` is refused with 400 at `/appendBody`.
    let both = client
        .patch(
            &entry,
            &json!({"readAt": second.value()["updatedAt"], "body": body("a"), "appendBody": body("b")}),
        )
        .await;
    both.check_bad_request();
    assert_eq!(both.refused_at(), "/appendBody", "{}", both.text());

    // Open collaborative room: pending edits flush first, then appendBody adds after them
    // and resets the room to the combined document.
    let mut peer = crate::rooms::Peer::connect(client, 202).await;
    peer.joined(&own, &entry_id).await;
    peer.write_acked("방에서 쓴 줄").await;
    let appended = client
        .patch(&entry, &json!({"appendBody": body("초안 덧붙임")}))
        .await;
    appended.check_ok();
    assert_eq!(
        appended.value()["body"],
        json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "첫 문단"}]},
            {"type": "paragraph", "content": [{"type": "text", "text": "둘째 문단"}]},
            {"type": "paragraph", "content": [{"type": "text", "text": "방에서 쓴 줄"}]},
            {"type": "paragraph", "content": [{"type": "text", "text": "초안 덧붙임"}]},
        ]})
    );
    peer.leave().await;

    assert_append_body_limits(client, &diary).await;
    th.teardown().await;
}

/// Deleting a completely empty entry (no body, no mood, no weather, no place) purges it
/// directly without leaving a shell in the diary trash, while entries carrying any body
/// or metadata still move to the trash and restore cleanly.
#[tokio::test]
async fn deleting_an_empty_entry_purges_it_while_non_empty_entries_go_to_trash() {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let diary = a_diary(client, &ws).await;

    // 1. Fresh empty entry -> purged directly.
    let blank = written(client, &diary, &json!({"date": "2024-07-01"})).await;
    let blank_id = text_of(&blank, "id");
    let blank_at = format!("{diary}/entries/{blank_id}");
    client
        .delete(&blank_at)
        .await
        .check_status(StatusCode::NO_CONTENT);
    assert_eq!(
        client.get(&format!("{diary}/trash/entries")).await.value(),
        json!([])
    );
    client
        .post(&format!("{blank_at}/restore"), &json!({}))
        .await
        .check_not_found();

    // 2. Entry written then cleared completely (body = null) -> purged directly, including versions.
    let cleared = written(client, &diary, &json!({"date": "2024-07-02"})).await;
    let cleared_id = text_of(&cleared, "id");
    let cleared_at = format!("{diary}/entries/{cleared_id}");
    let with_text = write_body(client, &cleared_at, &cleared, "지울 글").await;
    client
        .patch(
            &cleared_at,
            &json!({"readAt": with_text["updatedAt"], "body": Value::Null}),
        )
        .await
        .check_ok();
    client
        .delete(&cleared_at)
        .await
        .check_status(StatusCode::NO_CONTENT);
    assert_eq!(
        client.get(&format!("{diary}/trash/entries")).await.value(),
        json!([])
    );
    let left_versions = th
        .app
        .store()
        .system()
        .texts_unchecked(&format!(
            "SELECT CAST(count(*) AS TEXT) FROM body_versions WHERE block_id = '{cleared_id}'"
        ))
        .await
        .expect("count body_versions");
    assert_eq!(left_versions, ["0"], "purged entry leaves no body versions");

    // 3. Entries with ONLY mood, weather, place, or body still move to trash and restore.
    let cases = [
        (
            "2024-07-03",
            json!({"date": "2024-07-03", "mood": "good"}),
            false,
        ),
        (
            "2024-07-04",
            json!({"date": "2024-07-04", "weather": "맑음"}),
            false,
        ),
        (
            "2024-07-05",
            json!({"date": "2024-07-05", "place": "부산"}),
            false,
        ),
        ("2024-07-06", json!({"date": "2024-07-06"}), true),
    ];
    for (date, create_body, add_text) in cases {
        let made = written(client, &diary, &create_body).await;
        let id = text_of(&made, "id");
        let at = format!("{diary}/entries/{id}");
        if add_text {
            write_body(client, &at, &made, "남겨야 할 본문").await;
        }
        client
            .delete(&at)
            .await
            .check_status(StatusCode::NO_CONTENT);
        let trash = client.get(&format!("{diary}/trash/entries")).await.value();
        assert!(
            trash
                .as_array()
                .is_some_and(|items| items.iter().any(|item| item["id"] == json!(id))),
            "{date}: expected non-empty entry in trash: {trash}"
        );
        client
            .post(&format!("{at}/restore"), &json!({}))
            .await
            .check_ok();
    }
    th.teardown().await;
}

/// Builds a two-diary archive where diary 1 has an image in its entry body and diary 2
/// has an entry (optionally customized) and an optional file entry.
fn two_diary_archive_with_files(
    own_ws: &str,
    diary2_entries: &[Value],
    diary2_file: Option<(&str, &[u8])>,
) -> Vec<u8> {
    use std::io::Write as _;

    let d1 = json!({"type": "container", "data": {
        "id": "d1", "workspaceId": own_ws, "visibility": "private", "kind": "diary", "title": "First",
        "createdAt": 1, "updatedAt": 1, "fileNames": {"photo1.png": "photo1.png"},
    }});
    let e1 = json!({"type": "block", "data": {
        "id": "e1", "containerId": "d1", "parentId": "", "type": "entry", "title": "2024-08-01",
        "fields": {"date": "2024-08-01", "mood": "good"},
        "body": {"type": "doc", "content": [{"type": "image", "attrs": {"fileId": "photo1.png"}}]},
        "createdAt": 1, "updatedAt": 1,
    }});
    let d2 = json!({"type": "container", "data": {
        "id": "d2", "workspaceId": own_ws, "visibility": "private", "kind": "diary", "title": "Second",
        "createdAt": 1, "updatedAt": 1,
    }});
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer.start_file("version.json", options).expect("version");
    writer
        .write_all(br#"{"version":1,"date":1}"#)
        .expect("write version");
    writer
        .start_file("d1/container.jsonl", options)
        .expect("d1");
    writeln!(writer, "{d1}").expect("d1");
    writeln!(writer, "{e1}").expect("e1");
    writer.start_file("d1/photo1.png", options).expect("photo1");
    writer.write_all(b"png-bytes-1").expect("write photo1");
    writer
        .start_file("d2/container.jsonl", options)
        .expect("d2");
    writeln!(writer, "{d2}").expect("d2");
    for entry in diary2_entries {
        writeln!(writer, "{entry}").expect("d2 entry");
    }
    if let Some((name, bytes)) = diary2_file {
        writer
            .start_file(format!("d2/{name}"), options)
            .expect("d2 file");
        writer.write_all(bytes).expect("write d2 file");
    }
    writer.finish().expect("finish").into_inner()
}

async fn diary_rollback_snapshot(th: &TestServer) -> (i64, i64, i64, i64, usize, usize) {
    use futures_util::TryStreamExt as _;

    let count_sql = async |sql: &str| -> i64 {
        th.app
            .store()
            .system()
            .texts_unchecked(sql)
            .await
            .expect("query")[0]
            .parse()
            .expect("int")
    };
    let stages = std::fs::read_dir(th.app.config().tmp_dir()).map_or(0, |read| {
        read.filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("import-stage-"))
            .count()
    });
    let stored_files = th
        .app
        .files()
        .list()
        .try_collect::<Vec<_>>()
        .await
        .expect("list files")
        .len();
    (
        count_sql("SELECT CAST(count(*) AS TEXT) FROM containers WHERE kind = 'diary'").await,
        count_sql("SELECT CAST(count(*) AS TEXT) FROM diary_days").await,
        count_sql("SELECT CAST(count(*) AS TEXT) FROM files").await,
        count_sql("SELECT CAST(count(*) AS TEXT) FROM loose_objects").await,
        stages,
        stored_files,
    )
}

fn test_diary_and_entry(own_ws: &str, title: &str, date: &str) -> (Container, Block) {
    let diary = Container {
        id: new_id(),
        workspace_id: own_ws.to_owned(),
        visibility: ContainerVisibility::Private,
        kind: ContainerKind::Diary,
        title: title.to_owned(),
        ..Container::default()
    };
    let entry = Block {
        id: new_id(),
        container_id: diary.id.clone(),
        block_type: BlockType::Entry,
        title: date.to_owned(),
        fields: json!({"date": date}).as_object().expect("obj").clone(),
        ..Block::default()
    };
    (diary, entry)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CommitRollbackCase {
    DuplicateEntryDate,
    DiaryLimit,
}

async fn assert_commit_time_diary_rollback(
    th: &TestServer,
    own_org_id: &str,
    own_ws: &str,
    me_id: &str,
    expected: (i64, i64, i64, i64, usize, usize),
    case: CommitRollbackCase,
) {
    let own_store = th.app.store().for_org(own_org_id).expect("own store");
    let (d1, e1) = test_diary_and_entry(own_ws, "TxFirst", "2024-08-05");
    let (d2, e2a) = test_diary_and_entry(own_ws, "TxSecond", "2024-08-06");
    let f1 = fvoci_model::FileRecord {
        id: new_id(),
        name: "photo1.png".to_owned(),
        stored_name: "stored-photo1.png".to_owned(),
        extension: ".png".to_owned(),
        size: 12,
        container_id: d1.id.clone(),
        path: format!("{own_org_id}/{}/stored-photo1.png", d1.id),
        created_at: 1,
        ..fvoci_model::FileRecord::default()
    };
    let mut blocks = vec![e1];
    if case == CommitRollbackCase::DuplicateEntryDate {
        let mut e2b = e2a.clone();
        e2b.id = new_id();
        blocks.push(e2a);
        blocks.push(e2b);
    }
    let mut combined = ContainersAndBlocks {
        containers: vec![d1, d2],
        blocks,
    };
    let err = own_store
        .create_imported_containers_blocks_files_and_members(&mut combined, &[f1], &[], me_id)
        .await
        .expect_err("commit transaction must fail and roll back");
    if case == CommitRollbackCase::DiaryLimit {
        assert!(
            err.to_string().contains("20"),
            "expected TooManyDiaries error, got: {err}"
        );
    }
    assert_eq!(
        diary_rollback_snapshot(th).await,
        expected,
        "commit-time rollback left zero partial diaries, diary_days, or files"
    );
}

async fn assert_storage_write_failure_cleans_staged_files(
    th: &TestServer,
    own_ws: &str,
    entry_line: &Value,
    before: (i64, i64, i64, i64, usize, usize),
) {
    let two_files = two_diary_archive_with_files(
        own_ws,
        std::slice::from_ref(entry_line),
        Some(("photo2.png", b"png-bytes-2")),
    );
    th.app
        .files()
        .faults()
        .expect("test storage faults")
        .fail_after(fvoci_server::storage::FaultyOp::Write, 1, 1);
    crate::export::import(&th.client, own_ws, &two_files)
        .await
        .check_status(StatusCode::INTERNAL_SERVER_ERROR);
    let after_fault = diary_rollback_snapshot(th).await;
    assert_eq!(
        (
            after_fault.0,
            after_fault.1,
            after_fault.2,
            after_fault.4,
            after_fault.5
        ),
        (0, 0, 0, 0, 0),
        "diary 1 object, DB rows, and staging dir must all be gone: {after_fault:?}"
    );
    // The 1 remaining loose_objects row is the unsettled doomed mark for failed photo2.png;
    // sweeping after the write settle window removes it cleanly too.
    assert_eq!(after_fault.3, 1, "only unsettled mark for failed write");
    crate::loose_objects::remove_doomed(
        th,
        fvoci_server::auth::now_millis() + 2 * fvoci_server::app::WRITING_GRACE_MILLIS,
    )
    .await;
    assert_eq!(diary_rollback_snapshot(th).await, before, "after sweep");
}

/// Multi-diary archive import failure (second diary violating limits, duplicate/future
/// date, post-staging storage write failure, or DB commit-time failure) rolls back the
/// first diary, all blocks, `diary_days`, `files`, staged `FileBackend` objects,
/// `loose_objects`, and temporary staging directories.
#[tokio::test]
async fn multi_diary_archive_failure_rolls_back_all_diaries_and_staged_files() {
    let (th, ws) = in_a_space().await;
    let client = &th.client;
    let (_, own) = crate::own_space::workspaces_and_own(client).await;
    let own_ws = own.id.clone();

    let entry_line = |id: &str, date: &str| {
        json!({"type": "block", "data": {
            "id": id, "boardId": "d2", "parentId": "", "type": "entry", "title": date,
            "fields": {"date": date}, "createdAt": 1, "updatedAt": 1,
        }})
    };

    // 1. Diary 2 fails due to duplicate date, >2 days future date, or corrupt ZIP file.
    let before = diary_rollback_snapshot(&th).await;
    let dup_dates = two_diary_archive_with_files(
        &own_ws,
        &[
            entry_line("e2a", "2024-08-02"),
            entry_line("e2b", "2024-08-02"),
        ],
        None,
    );
    crate::export::import(client, &own_ws, &dup_dates)
        .await
        .check_bad_request();
    assert_eq!(diary_rollback_snapshot(&th).await, before, "duplicate date");

    let future_date =
        two_diary_archive_with_files(&own_ws, &[entry_line("e2f", "9999-12-31")], None);
    crate::export::import(client, &own_ws, &future_date)
        .await
        .check_bad_request();
    assert_eq!(diary_rollback_snapshot(&th).await, before, "future date");

    let mut bad_zip = two_diary_archive_with_files(
        &own_ws,
        &[entry_line("e2ok", "2024-08-02")],
        Some(("d2file.png", b"payload-corrupt-me")),
    );
    let pos = bad_zip
        .windows(b"payload-corrupt-me".len())
        .position(|w| w == b"payload-corrupt-me")
        .expect("payload bytes");
    bad_zip[pos] = b'X';
    crate::export::import(client, &own_ws, &bad_zip)
        .await
        .check_bad_request();
    assert_eq!(diary_rollback_snapshot(&th).await, before, "corrupt zip");

    // 2. Post-staging FileBackend failure cleans diary 1's written object + staging dir.
    let ok_entry = entry_line("e2ok", "2024-08-02");
    assert_storage_write_failure_cleans_staged_files(&th, &own_ws, &ok_entry, before).await;

    // 3. Commit-time DB rollback after containers, files, and diary_days are inserted.
    for _ in 0..DIARIES_PER_SPACE - 2 {
        a_diary(client, &ws).await;
    }
    let me = client.me().await;
    let at_18 = diary_rollback_snapshot(&th).await;
    assert_commit_time_diary_rollback(
        &th,
        &own.org_id,
        &own_ws,
        &me.id,
        at_18,
        CommitRollbackCase::DuplicateEntryDate,
    )
    .await;

    // 4. Space at DIARIES_PER_SPACE - 1 (19): HTTP import returns too_many_diaries and
    // commit-time limit race rolls back d1 cleanly.
    a_diary(client, &ws).await;
    let at_19 = diary_rollback_snapshot(&th).await;
    let valid_two = two_diary_archive_with_files(&own_ws, &[ok_entry], None);
    let full = crate::export::import(client, &own_ws, &valid_two).await;
    full.check_bad_request();
    assert_eq!(full.error_reason(), "too_many_diaries", "{}", full.text());
    assert_eq!(diary_rollback_snapshot(&th).await, at_19, "limit refusal");
    assert_commit_time_diary_rollback(
        &th,
        &own.org_id,
        &own_ws,
        &me.id,
        at_19,
        CommitRollbackCase::DiaryLimit,
    )
    .await;
    th.teardown().await;
}
