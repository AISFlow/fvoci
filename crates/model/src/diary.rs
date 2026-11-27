//! Diaries: the entry of a day as its operations give and take it, the day it is of,
//! and the days of a diary that are written.
//!
//! A diary is a container of the kind `diary` ([`crate::ContainerKind::Diary`]) in the
//! workspace of a user's own space. Its entries are blocks of the type `entry`
//! ([`crate::BlockType::Entry`]), one a day: the text of an entry is the block's body,
//! and its day, its mood, its weather and its place are among the block's fields
//! ([`ENTRY_DATE_FIELD`], [`ENTRY_MOOD_FIELD`], [`ENTRY_WEATHER_FIELD`],
//! [`ENTRY_PLACE_FIELD`]). The block's title is its day too, written as the field is,
//! so that what lists blocks by their titles (a search, the trash) names an entry by
//! its day.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::body::{Body, BodyChange};
use crate::refusal::{RefusalReason, Refusals};
use crate::{Block, BlockPatch, JsonMap};

/// The field of an entry that holds its day ([`DiaryDate`]): a string, `2026-11-21`.
pub const ENTRY_DATE_FIELD: &str = "date";

/// The field of an entry that holds its mood ([`Mood`]): a string; absent for none.
pub const ENTRY_MOOD_FIELD: &str = "mood";

/// The field of an entry that holds the weather of its day, as its writer put it: a
/// string; absent for none.
pub const ENTRY_WEATHER_FIELD: &str = "weather";

/// The field of an entry that holds where its day was spent, as its writer put it: a
/// string; absent for none.
pub const ENTRY_PLACE_FIELD: &str = "place";

/// The field of an entry that holds when its body last changed (or when the entry was
/// made, if its body has not changed since), in milliseconds since the Unix epoch.
/// Managed by the store so conditional body writes compare `readAt` against body
/// changes rather than metadata patches (`mood`, `weather`, `place`).
pub const ENTRY_BODY_UPDATED_AT_FIELD: &str = "bodyUpdatedAt";

/// The most characters the weather of an entry has, and its place: a few words each,
/// shown on one line at the head of the entry.
pub const ENTRY_NOTE_MAX_CHARS: usize = 100;

/// The most diaries the workspace of a space has at once, those in the trash not
/// counted. A diary is a volume a person keeps (a journal, a training log): the limit
/// is far above what one keeps, and holds what a request can make of them.
pub const DIARIES_PER_SPACE: u32 = 20;

/// The most entries a diary has at once, those in the trash not counted: a hundred
/// years of days and some.
pub const ENTRIES_PER_DIARY: u32 = 40_000;

/// How many days past the server's own day (UTC) the day of a new entry may be. The
/// server does not know what day it is where the writer is, and no place is more than
/// a day ahead of UTC: two days leave a device whose clock runs fast its room and
/// keep a diary from being filled with days far to come.
pub const ENTRY_DAYS_AHEAD: i32 = 2;

/// How many days of a diary one list of its written days spans at most: a year, the
/// most a calendar shows at once. A diary is not read whole for its days.
pub const DAYS_PER_LIST: u16 = 366;

/// A day of the calendar, as a diary's entry is of one: no time and no time zone, since
/// the day is the one its writer's device said it was. Written `2026-11-21`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DiaryDate(jiff::civil::Date);

/// Error returned when a text is no day of the calendar as [`DiaryDate`] writes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("a day is written as `2026-11-21`, and is a day of the calendar")]
pub struct ParseDiaryDateError;

impl DiaryDate {
    /// The day of `year`, `month` and `day`, if the calendar has it: the year from 1 to
    /// 9999, and 29 February in a leap year only.
    #[must_use]
    pub fn new(year: i16, month: i8, day: i8) -> Option<Self> {
        if !(1..=9999).contains(&year) {
            return None;
        }
        jiff::civil::Date::new(year, month, day).ok().map(Self)
    }

    /// The day it is at `millis` (since the Unix epoch) in UTC; `None` for a time
    /// outside the years a day may be of, which no clock gives.
    #[must_use]
    pub fn of_utc_millis(millis: i64) -> Option<Self> {
        let time = jiff::Timestamp::from_millisecond(millis).ok()?;
        let date = jiff::tz::TimeZone::UTC.to_datetime(time).date();
        Self::new(date.year(), date.month(), date.day())
    }

    /// The day `days` after this one (before it, for a negative number); `None` past
    /// the years a day may be of.
    #[must_use]
    pub fn plus_days(self, days: i32) -> Option<Self> {
        let span = jiff::Span::new().try_days(days).ok()?;
        let date = self.0.checked_add(span).ok()?;
        Self::new(date.year(), date.month(), date.day())
    }

    /// Its month and its day without its year, `11-21`: what the entries of one page of
    /// a diary share, each of another year. 29 February is a day of its own, which only
    /// leap years have.
    #[must_use]
    pub fn month_day(self) -> MonthDay {
        MonthDay {
            month: self.0.month(),
            day: self.0.day(),
        }
    }

    /// Its year.
    #[must_use]
    pub fn year(self) -> i16 {
        self.0.year()
    }

    /// Underlying civil date for crate-internal calendar calculations.
    #[must_use]
    pub(crate) const fn civil_date(self) -> jiff::civil::Date {
        self.0
    }

    /// Unix epoch date (`1970-01-01`), used as an infallible fallback outside years 1..=9999.
    pub(crate) const UNIX_EPOCH: Self = Self(jiff::civil::date(1970, 1, 1));
}

impl fmt::Display for DiaryDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let date = self.0;
        write!(
            f,
            "{:04}-{:02}-{:02}",
            date.year(),
            date.month(),
            date.day()
        )
    }
}

impl FromStr for DiaryDate {
    type Err = ParseDiaryDateError;

    /// Reads a day exactly as [`DiaryDate`] displays it: four digits, two and two, with
    /// `-` between. Nothing else a date is written as elsewhere is taken, so that a day
    /// is stored in one form and days compare as their texts do.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (year, month_day) = text.split_at_checked(4).ok_or(ParseDiaryDateError)?;
        let month_day: MonthDay = month_day
            .strip_prefix('-')
            .ok_or(ParseDiaryDateError)?
            .parse()
            .map_err(|_| ParseDiaryDateError)?;
        let year = digits(year).ok_or(ParseDiaryDateError)?;
        let year = i16::try_from(year).map_err(|_| ParseDiaryDateError)?;
        Self::new(year, month_day.month, month_day.day).ok_or(ParseDiaryDateError)
    }
}

impl Serialize for DiaryDate {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for DiaryDate {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// The number that `text` is if it is digits and nothing else.
fn digits(text: &str) -> Option<u32> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// A month and a day of it without a year, as the page of a diary is of one: `11-21`.
/// 29 February is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonthDay {
    month: i8,
    day: i8,
}

/// Error returned when a text is no month and day as [`MonthDay`] writes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("a month and its day are written as `11-21`, and are a day some year has")]
pub struct ParseMonthDayError;

impl fmt::Display for MonthDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}-{:02}", self.month, self.day)
    }
}

impl FromStr for MonthDay {
    type Err = ParseMonthDayError;

    /// Reads a month and a day exactly as [`MonthDay`] displays them, and takes those a
    /// leap year has: `02-29` is one, `02-30` none.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (month, day) = text.split_once('-').ok_or(ParseMonthDayError)?;
        if month.len() != 2 || day.len() != 2 {
            return Err(ParseMonthDayError);
        }
        let part = |text: &str| digits(text).and_then(|number| i8::try_from(number).ok());
        let (month, day) = part(month).zip(part(day)).ok_or(ParseMonthDayError)?;
        // A leap year has every month and day there is.
        jiff::civil::Date::new(2000, month, day)
            .map(|_| Self { month, day })
            .map_err(|_| ParseMonthDayError)
    }
}

/// How the day of an entry was, as its writer marks it: one of five, or none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Mood {
    /// The best of days.
    Great,
    /// A good day.
    Good,
    /// Neither good nor bad.
    Okay,
    /// A bad day.
    Bad,
    /// The worst of days.
    Awful,
}

impl Mood {
    /// Every mood there is, from the best to the worst.
    pub const ALL: [Self; 5] = [Self::Great, Self::Good, Self::Okay, Self::Bad, Self::Awful];

    /// Returns the wire and database form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Great => "great",
            Self::Good => "good",
            Self::Okay => "okay",
            Self::Bad => "bad",
            Self::Awful => "awful",
        }
    }
}

impl fmt::Display for Mood {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a text names no mood.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("the text names no mood")]
pub struct ParseMoodError;

impl FromStr for Mood {
    type Err = ParseMoodError;

    /// Reads the wire and database form ([`Mood::as_str`]), exactly.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|mood| mood.as_str() == text)
            .ok_or(ParseMoodError)
    }
}

/// Whether `value` is the day of an entry: a string [`DiaryDate`] reads.
pub(crate) fn is_date_value(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| text.parse::<DiaryDate>().is_ok())
}

/// Whether `value` is the mood of an entry: a string that names one.
pub(crate) fn is_mood_value(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| text.parse::<Mood>().is_ok())
}

/// Whether `value` is the weather or the place of an entry: a string of at most
/// [`ENTRY_NOTE_MAX_CHARS`] characters, none of them U+0000.
pub(crate) fn is_note_value(value: &Value) -> bool {
    value.as_str().is_some_and(is_note)
}

/// Whether `value` is the body-change timestamp of an entry: a non-negative integer in
/// milliseconds since the Unix epoch.
pub(crate) fn is_body_updated_at_value(value: &Value) -> bool {
    value.as_i64().is_some_and(|time| time >= 0)
}

fn is_note(text: &str) -> bool {
    text.chars().count() <= ENTRY_NOTE_MAX_CHARS && !text.contains('\0')
}

/// The day of the entry that `block` is, if it has one that is a day.
#[must_use]
pub fn entry_date(block: &Block) -> Option<DiaryDate> {
    block
        .fields
        .get(ENTRY_DATE_FIELD)
        .and_then(Value::as_str)
        .and_then(|text| text.parse().ok())
}

/// The mood of the entry that `block` is, if it has one.
#[must_use]
pub fn entry_mood(block: &Block) -> Option<Mood> {
    block
        .fields
        .get(ENTRY_MOOD_FIELD)
        .and_then(Value::as_str)
        .and_then(|text| text.parse().ok())
}

/// When the body of the entry `block` last changed, in milliseconds since the Unix
/// epoch. For an entry stored before this field was kept: `updated_at` if it has a body
/// (so stale writers are still refused), or `created_at` if it has none.
#[must_use]
pub fn entry_body_updated_at(block: &Block) -> i64 {
    block
        .fields
        .get(ENTRY_BODY_UPDATED_AT_FIELD)
        .and_then(Value::as_i64)
        .unwrap_or(if block.body.is_some() {
            block.updated_at
        } else {
            block.created_at
        })
}

/// Whether the entry `block` has nothing written in it: no body, no mood, no weather
/// and no place.
#[must_use]
pub fn is_empty_entry(block: &Block) -> bool {
    block.body.is_none()
        && entry_mood(block).is_none()
        && text_field(&block.fields, ENTRY_WEATHER_FIELD).is_empty()
        && text_field(&block.fields, ENTRY_PLACE_FIELD).is_empty()
}

fn text_field(fields: &JsonMap, field: &str) -> String {
    fields
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// Whether a diary is made whatever the space has, or only as its first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiaryMaking {
    /// One more diary, up to the limit of a space.
    Another,
    /// A diary if the space has none: where it has one, that one is answered and none
    /// is made. What opens "the diary" of a space that may have none yet asks so, and
    /// two that ask at once get the same one.
    IfNone,
}

/// The entry of a day of a diary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiaryEntry {
    /// The entry's identifier: what its room is joined by (`joinDoc`).
    pub id: String,
    /// The identifier of the diary the entry is of.
    pub diary_id: String,
    /// The day the entry is of, `2026-11-21`: the day it was where its writer was. It
    /// is what the entry was made with and never changes; a diary has one entry a day.
    #[schema(value_type = String)]
    pub date: String,
    /// How the day was; `null` for none.
    pub mood: Option<Mood>,
    /// The weather of the day as its writer put it; empty for none.
    pub weather: String,
    /// Where the day was spent as its writer put it; empty for none.
    pub place: String,
    /// The entry's body; `null` for an entry without one.
    pub body: Option<Body>,
    /// The identifier of the user who made the entry.
    pub created_by: String,
    /// The identifier of the user who last changed the entry.
    pub modified_by: String,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Last modification time in milliseconds since the Unix epoch: what `readAt` of a
    /// change of the entry names.
    pub updated_at: i64,
}

impl From<Block> for DiaryEntry {
    /// The entry that the block of an entry is.
    fn from(block: Block) -> Self {
        Self {
            date: text_field(&block.fields, ENTRY_DATE_FIELD),
            mood: entry_mood(&block),
            weather: text_field(&block.fields, ENTRY_WEATHER_FIELD),
            place: text_field(&block.fields, ENTRY_PLACE_FIELD),
            id: block.id,
            diary_id: block.container_id,
            body: block.body,
            created_by: block.created_by,
            modified_by: block.modified_by,
            created_at: block.created_at,
            updated_at: block.updated_at,
        }
    }
}

/// A day of a diary that has an entry, as the calendar of the diary marks it: without
/// the entry's text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiaryDay {
    /// The day, `2026-11-21`.
    #[schema(value_type = String)]
    pub date: DiaryDate,
    /// The identifier of the entry of the day.
    pub entry_id: String,
    /// How the day was; left out for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mood: Option<Mood>,
}

/// Days of a diary one after the other, each with an entry, with a day without one (or
/// the edge of the calendar) before the first and after the last: what a count of the
/// days written in a row is made of. Which run reaches today, and so how long the
/// writer's streak is now, the client says: the server never says what today is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DiaryRun {
    /// The first day of the run, `2026-11-01`.
    #[schema(value_type = String)]
    pub from: DiaryDate,
    /// The last day of the run, `2026-11-21`.
    #[schema(value_type = String)]
    pub to: DiaryDate,
    /// How many days the run has, both of its ends counted.
    pub days: u32,
}

/// An entry in the trash, as the trash of a diary lists it: `restoreDiaryEntry` brings
/// it back to its day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeletedDiaryEntry {
    /// The entry's identifier.
    pub id: String,
    /// The day the entry is of, `2026-11-21`.
    #[schema(value_type = String)]
    pub date: String,
    /// How the day was; left out for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mood: Option<Mood>,
    /// When the entry was deleted, in milliseconds since the Unix epoch.
    pub deleted_at: i64,
    /// The identifier of the user who deleted it; empty if that user is gone.
    pub deleted_by: String,
}

/// An entry to make.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewDiaryEntry {
    /// The day the entry is of, `2026-11-21`: the day it is where the writer is, as
    /// their device says, or an earlier day written up afterwards. A day of the
    /// calendar from the year 1 to the year 9999, and at most two days past the
    /// server's own day (UTC).
    #[schema(value_type = String)]
    pub date: String,
    /// How the day was. Left out or `null`, none.
    #[serde(default)]
    pub mood: Option<Mood>,
    /// The weather of the day, at most 100 characters. Left out, none.
    #[serde(default)]
    pub weather: String,
    /// Where the day was spent, at most 100 characters. Left out, none.
    #[serde(default)]
    pub place: String,
}

const DATE_INVALID: &str = "a day is written as `2026-11-21`, and is a day of the calendar";
const DATE_TOO_FAR: &str = "the day is more than two days past today";
const NOTE_TOO_LONG: &str = "at most 100 characters";
const NOTE_WITH_NUL: &str = "the text holds the character U+0000";
const READ_AT_MISSING: &str = "a new body is sent with the `updatedAt` the entry was read at";
const BODY_AND_APPEND: &str = "`body` and `appendBody` cannot be sent in the same patch";

/// Refuses a weather or a place that is too long or holds U+0000.
fn refuse_note(refused: &mut Refusals, member: &str, text: Option<&str>) {
    let Some(text) = text else {
        return;
    };
    if text.chars().count() > ENTRY_NOTE_MAX_CHARS {
        refused.refuse(member, RefusalReason::TooLong, NOTE_TOO_LONG);
    } else if text.contains('\0') {
        refused.refuse(member, RefusalReason::InvalidCharacters, NOTE_WITH_NUL);
    }
}

impl NewDiaryEntry {
    /// Checks the shape of the entry as a request sends it, all of it, and returns its
    /// day. `today` is the server's own day.
    ///
    /// # Errors
    ///
    /// Returns every member that is refused: a `date` that is no day of the calendar,
    /// or more than [`ENTRY_DAYS_AHEAD`] days past `today`, and a weather or a place
    /// that is too long or holds U+0000.
    pub fn check_request(&self, today: DiaryDate) -> Result<DiaryDate, Refusals> {
        let mut refused = Refusals::new();
        let date = self.date.parse::<DiaryDate>().ok();
        match date {
            None => refused.refuse("date", RefusalReason::InvalidFormat, DATE_INVALID),
            Some(date) if !date.is_within_reach_of(today) => {
                refused.refuse("date", RefusalReason::Invalid, DATE_TOO_FAR);
            }
            Some(_) => {}
        }
        refuse_note(&mut refused, "weather", Some(&self.weather));
        refuse_note(&mut refused, "place", Some(&self.place));
        refused.into_result()?;
        // A day that is none was refused above.
        date.ok_or_else(Refusals::new)
    }

    /// The fields of the block of the entry, for its day `date`.
    #[must_use]
    pub fn into_fields(self, date: DiaryDate) -> JsonMap {
        let mut fields = JsonMap::new();
        fields.insert(ENTRY_DATE_FIELD.to_owned(), Value::from(date.to_string()));
        if let Some(mood) = self.mood {
            fields.insert(ENTRY_MOOD_FIELD.to_owned(), Value::from(mood.as_str()));
        }
        for (field, text) in [
            (ENTRY_WEATHER_FIELD, self.weather),
            (ENTRY_PLACE_FIELD, self.place),
        ] {
            if !text.is_empty() {
                fields.insert(field.to_owned(), Value::from(text));
            }
        }
        fields
    }
}

impl DiaryDate {
    /// Whether the days from this one to `to` are no more than are listed at once
    /// ([`DAYS_PER_LIST`]). A range that ends before it begins is of no days.
    #[must_use]
    pub fn lists_to(self, to: Self) -> bool {
        self.plus_days(i32::from(DAYS_PER_LIST) - 1)
            .is_none_or(|last| to <= last)
    }

    /// Whether an entry of this day may be made when the server's own day is `today`:
    /// any day up to [`ENTRY_DAYS_AHEAD`] days past it.
    #[must_use]
    pub fn is_within_reach_of(self, today: Self) -> bool {
        today
            .plus_days(ENTRY_DAYS_AHEAD)
            .is_none_or(|latest| self <= latest)
    }
}

/// What a patch does to the mood of an entry. Its JSON form is the member `mood` of the
/// patch: left out keeps the mood, `null` takes it away, and a mood replaces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MoodChange {
    /// The mood stays as it is.
    #[default]
    Keep,
    /// The entry has no mood.
    Clear,
    /// The mood becomes this one.
    Set(Mood),
}

impl Serialize for MoodChange {
    /// The mood for [`MoodChange::Set`], and `null` for the others: the schema of a
    /// patch writes the member's default with it.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Keep | Self::Clear => serializer.serialize_none(),
            Self::Set(mood) => mood.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for MoodChange {
    /// Reads the member when it is there; a member that is left out is
    /// [`MoodChange::Keep`] by its `default`.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Option::<Mood>::deserialize(deserializer)?.map_or(Self::Clear, Self::Set))
    }
}

/// A change of an entry. Absent members leave the entry as it is. Its day does not
/// change.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct DiaryEntryPatch {
    /// The new mood; `null` for none.
    #[schema(value_type = Option<Mood>)]
    pub mood: MoodChange,
    /// The new weather, at most 100 characters; empty for none.
    pub weather: Option<String>,
    /// The new place, at most 100 characters; empty for none.
    pub place: Option<String>,
    /// The new body; `null` takes the body away. Sent only with `readAt`, and not
    /// together with `appendBody`.
    #[schema(value_type = Option<Body>)]
    pub body: BodyChange,
    /// A document whose blocks are appended after the entry's current body inside one
    /// server transaction without replacing what is already there. Cannot be sent
    /// together with `body`. `null` or an empty document appends nothing. Refused (400)
    /// if the combined body exceeds the body limits (`BODY_MAX_BYTES`,
    /// `BODY_MAX_NODES`, `BODY_MAX_DEPTH`). Does not require `readAt`; if `readAt` is
    /// also sent, it is checked against the entry's body timestamp before appending.
    pub append_body: Option<Body>,
    /// The `updatedAt` of the entry as the client read it: checked against when the
    /// entry's body last changed, so metadata-only patches (`mood`, `weather`, `place`)
    /// do not cause 409 conflicts while any change to the body does. Needed with `body`,
    /// and checked whenever it is sent.
    pub read_at: Option<i64>,
}

impl DiaryEntryPatch {
    /// Whether this patch replaces, clears, or appends to the entry's body.
    #[must_use]
    pub fn has_body_write(&self) -> bool {
        !self.body.is_keep() || self.append_body.is_some()
    }

    /// Checks the shape of the patch as a request sends it, all of it.
    ///
    /// # Errors
    ///
    /// Returns every member that is refused: a weather or a place that is too long or
    /// holds U+0000, a new body without `readAt`, and sending `body` together with
    /// `appendBody`.
    pub fn check_request(&self) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        if !self.body.is_keep() && self.append_body.is_some() {
            refused.refuse("appendBody", RefusalReason::Invalid, BODY_AND_APPEND);
        }
        if !self.body.is_keep() && self.read_at.is_none() {
            refused.refuse("readAt", RefusalReason::Required, READ_AT_MISSING);
        }
        refuse_note(&mut refused, "weather", self.weather.as_deref());
        refuse_note(&mut refused, "place", self.place.as_deref());
        refused.into_result()
    }

    /// The patch of the entry's block that does what this patch says.
    #[must_use]
    pub fn into_block_patch(self) -> BlockPatch {
        let mut patch = BlockPatch {
            body: self.body,
            read_at: self.read_at,
            ..BlockPatch::default()
        };
        let mood = match self.mood {
            MoodChange::Keep => None,
            MoodChange::Clear => Some(None),
            MoodChange::Set(mood) => Some(Some(mood.as_str().to_owned())),
        };
        let notes = [
            (ENTRY_MOOD_FIELD, mood),
            (
                ENTRY_WEATHER_FIELD,
                self.weather
                    .map(|text| Some(text).filter(|text| !text.is_empty())),
            ),
            (
                ENTRY_PLACE_FIELD,
                self.place
                    .map(|text| Some(text).filter(|text| !text.is_empty())),
            ),
        ];
        for (field, change) in notes {
            match change {
                None => {}
                Some(Some(text)) => {
                    patch
                        .updated_fields
                        .insert(field.to_owned(), Value::from(text));
                }
                Some(None) => patch.deleted_fields.push(field.to_owned()),
            }
        }
        patch
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn day(text: &str) -> DiaryDate {
        text.parse().expect("a day")
    }

    #[test]
    fn a_day_is_read_in_one_form_and_is_a_day_of_the_calendar() {
        for text in ["2026-11-21", "2024-02-29", "0001-01-01", "9999-12-31"] {
            assert_eq!(day(text).to_string(), text);
        }
        for text in [
            "",
            "2026-11-31",
            "2026-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-11-00",
            "0000-01-01",
            "10000-01-01",
            "2026-1-1",
            "20261121",
            "2026/11/21",
            "2026-11-21T00:00:00Z",
            " 2026-11-21",
            "2026-11-21 ",
            "+026-11-21",
            "2026-+1-21",
            "２０２６-11-21",
        ] {
            assert!(text.parse::<DiaryDate>().is_err(), "{text:?} is no day");
        }
    }

    #[test]
    fn the_twenty_ninth_of_february_is_a_month_and_day_of_its_own() {
        assert_eq!(day("2024-02-29").month_day().to_string(), "02-29");
        assert_ne!(day("2024-02-29").month_day(), day("2025-02-28").month_day());
        assert_eq!(
            "02-29".parse::<MonthDay>().map(|at| at.to_string()),
            Ok("02-29".to_owned())
        );
        for text in [
            "02-30", "2-29", "13-01", "00-01", "11-00", "1121", "11-21-", "+1-21",
        ] {
            assert!(text.parse::<MonthDay>().is_err(), "{text:?}");
        }
    }

    #[test]
    fn a_new_entry_is_of_a_day_up_to_two_days_past_the_servers() {
        let today = day("2026-11-21");
        let new = |date: &str| NewDiaryEntry {
            date: date.to_owned(),
            mood: None,
            weather: String::new(),
            place: String::new(),
        };
        for date in ["2026-11-21", "2026-11-23", "2025-11-21", "1987-03-02"] {
            assert_eq!(new(date).check_request(today), Ok(day(date)), "{date}");
        }
        for (date, reason) in [
            ("2026-11-24", RefusalReason::Invalid),
            ("2027-11-21", RefusalReason::Invalid),
            ("2026-02-30", RefusalReason::InvalidFormat),
            ("tomorrow", RefusalReason::InvalidFormat),
        ] {
            let refused = new(date).check_request(today).expect_err(date);
            let said: Vec<_> = refused
                .iter()
                .map(|refusal| (refusal.pointer.as_str(), refusal.reason))
                .collect();
            assert_eq!(said, [("/date", reason)], "{date}");
        }
        // The last day there is has no day two days past it to be held to.
        assert!(day("9999-12-31").is_within_reach_of(day("9999-12-30")));
    }

    #[test]
    fn the_notes_of_an_entry_are_short_and_hold_no_nul() {
        let today = day("2026-11-21");
        let long = "가".repeat(ENTRY_NOTE_MAX_CHARS + 1);
        let new = NewDiaryEntry {
            date: "2026-11-21".to_owned(),
            mood: Some(Mood::Good),
            weather: long.clone(),
            place: "a\0b".to_owned(),
        };
        let refused = new.check_request(today).expect_err("both are refused");
        let said: Vec<_> = refused
            .iter()
            .map(|refusal| (refusal.pointer.as_str(), refusal.reason))
            .collect();
        assert_eq!(
            said,
            [
                ("/weather", RefusalReason::TooLong),
                ("/place", RefusalReason::InvalidCharacters)
            ]
        );
        let fits = "가".repeat(ENTRY_NOTE_MAX_CHARS);
        let patch = DiaryEntryPatch {
            weather: Some(fits),
            place: Some(long),
            ..DiaryEntryPatch::default()
        };
        let refused = patch.check_request().expect_err("the place is refused");
        assert_eq!(refused.iter().count(), 1);
    }

    #[test]
    fn a_patch_tells_a_mood_left_out_from_one_taken_away() {
        let read =
            |value: Value| -> DiaryEntryPatch { serde_json::from_value(value).expect("a patch") };
        assert_eq!(read(json!({})).mood, MoodChange::Keep);
        assert_eq!(read(json!({"mood": null})).mood, MoodChange::Clear);
        assert_eq!(
            read(json!({"mood": "bad"})).mood,
            MoodChange::Set(Mood::Bad)
        );
        assert!(serde_json::from_value::<DiaryEntryPatch>(json!({"mood": "sleepy"})).is_err());
        assert!(serde_json::from_value::<DiaryEntryPatch>(json!({"date": "2026-11-22"})).is_err());

        let patch = read(json!({"mood": null, "weather": "", "place": "Seoul"})).into_block_patch();
        assert_eq!(
            patch.deleted_fields,
            [ENTRY_MOOD_FIELD, ENTRY_WEATHER_FIELD]
        );
        assert_eq!(
            Value::Object(patch.updated_fields),
            json!({"place": "Seoul"})
        );
        assert_eq!(patch.title, None);
    }

    #[test]
    fn an_entry_is_read_from_its_block() {
        let block = Block {
            id: "e".to_owned(),
            container_id: "d".to_owned(),
            block_type: crate::BlockType::Entry,
            title: "2026-11-21".to_owned(),
            fields: json!({"date": "2026-11-21", "mood": "great", "place": "home"})
                .as_object()
                .expect("an object")
                .clone(),
            ..Block::default()
        };
        assert_eq!(entry_date(&block), Some(day("2026-11-21")));
        let entry = DiaryEntry::from(block);
        assert_eq!(
            (
                entry.date.as_str(),
                entry.mood,
                entry.weather.as_str(),
                entry.place.as_str()
            ),
            ("2026-11-21", Some(Mood::Great), "", "home")
        );
    }

    #[test]
    fn empty_entry_detection_and_body_timestamp_fallback() {
        let mut block = Block {
            id: "e".to_owned(),
            container_id: "d".to_owned(),
            block_type: crate::BlockType::Entry,
            title: "2026-11-21".to_owned(),
            fields: json!({"date": "2026-11-21"})
                .as_object()
                .expect("object")
                .clone(),
            created_at: 100,
            updated_at: 250,
            ..Block::default()
        };
        assert!(is_empty_entry(&block));
        assert_eq!(entry_body_updated_at(&block), 100);

        block
            .fields
            .insert(ENTRY_WEATHER_FIELD.to_owned(), json!("맑음"));
        assert!(!is_empty_entry(&block));

        block
            .fields
            .insert(ENTRY_BODY_UPDATED_AT_FIELD.to_owned(), json!(180));
        assert_eq!(entry_body_updated_at(&block), 180);

        let body = Body::parse(json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "x"}]}
        ]}))
        .expect("valid")
        .expect("non-empty");
        let conflict = DiaryEntryPatch {
            body: BodyChange::Set(body.clone()),
            append_body: Some(body),
            read_at: Some(180),
            ..DiaryEntryPatch::default()
        };
        let refused = conflict.check_request().expect_err("body + appendBody");
        assert!(refused.iter().any(|r| r.pointer == "/appendBody"));
    }
}
