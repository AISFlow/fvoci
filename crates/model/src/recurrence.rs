//! Cards that repeat: a card of a board that has a rule of done (`doneWhen`) and a due
//! date property may say how it repeats ([`Recurrence`]). When it is completed, the
//! server makes the next card in the same write: its title, icon, parent card, values
//! and body, the value of the rule of done set back to one that is not done, the task
//! items of the body unchecked, and the due date moved on ([`Recurrence::next_due`]).
//! How it repeats goes with it to the next card; the completed one keeps the identifier
//! of the next (`recurredTo`), which the server alone writes. Setting how it repeats
//! again (undoing the completion sends it back) clears that link, so a card completed,
//! reopened and completed again makes one next card.
//!
//! Counted from the due date, the next cards of one card are a chain whose steps are
//! all counted from the due date of its first card ([`Recurrence::from`]), so that a
//! chain from the 31st keeps the ends of months, and one from the 29th of February
//! comes back to it in a leap year.

use jiff::civil::Date;
use jiff::{Span, Timestamp, Unit};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::JsonMap;
use crate::board_ops::DoneWhen;
use crate::body::Body;
use crate::card_property::{CardPropertyDefinition, PropertyType};
use crate::date_value::{LAST_MILLISECOND, is_calendar_date};
use crate::settings::TimeZone;

/// The member of a card's fields that says how it repeats.
pub const RECURRENCE_FIELD: &str = "recurrence";

/// The member of a completed card's fields that names the card made after it.
pub const RECURRED_TO_FIELD: &str = "recurredTo";

/// The most periods one step of a repeat may span: a year of days.
pub const MOST_PERIODS: u16 = 365;

/// How a card repeats: every `every` days, weeks, months or years, counted from its due
/// date or from the day it was completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", try_from = "RecurrenceShape")]
pub struct Recurrence {
    /// How many units apart: from 1 to 365.
    #[schema(minimum = 1, maximum = 365)]
    pub every: u16,
    /// The unit.
    pub unit: RecurrenceUnit,
    /// What the next due date is counted from.
    pub anchor: RecurrenceAnchor,
    /// The due date of the first card of the chain this card is of, which every step
    /// of a recurrence counted from the due date is counted from: the server hands it
    /// to the next card, and takes it away when how the card repeats is changed or
    /// its due date is. None for the first card; a date after the card's due date
    /// counts for none.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "date_text_of"
    )]
    #[schema(value_type = Option<String>, format = Date, nullable = false)]
    pub from: Option<Date>,
}

/// The unit a card repeats by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RecurrenceUnit {
    /// Days.
    Day,
    /// Weeks of seven days.
    Week,
    /// Calendar months: the 31st of a month of 30 days is its 30th.
    Month,
    /// Calendar years: the 29th of February of a year that has none is the 28th.
    Year,
}

/// What the next due date of a card that repeats is counted from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RecurrenceAnchor {
    /// The due date: the next is the first date after today a whole number of periods
    /// after it, so that a card completed late keeps its days.
    Due,
    /// The day it was completed, in the completer's time zone: the next is one period
    /// after it.
    Done,
}

/// A [`Recurrence`] as JSON has it, before its range is checked.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecurrenceShape {
    every: u16,
    unit: RecurrenceUnit,
    anchor: RecurrenceAnchor,
    #[serde(default)]
    from: Option<String>,
}

/// Why JSON is no [`Recurrence`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidRecurrence {
    /// `every` is out of its range.
    #[error("a card repeats every 1 to 365 days, weeks, months or years")]
    OutOfRange,
    /// `from` is no day of the calendar.
    #[error("a chain of cards that repeat counts from a calendar date, YYYY-MM-DD")]
    NotADate,
}

impl TryFrom<RecurrenceShape> for Recurrence {
    type Error = InvalidRecurrence;

    fn try_from(shape: RecurrenceShape) -> Result<Self, Self::Error> {
        if !(1..=MOST_PERIODS).contains(&shape.every) {
            return Err(InvalidRecurrence::OutOfRange);
        }
        let from = match shape.from {
            Some(text) if is_calendar_date(&text) => {
                Some(text.parse().map_err(|_| InvalidRecurrence::NotADate)?)
            }
            Some(_) => return Err(InvalidRecurrence::NotADate),
            None => None,
        };
        Ok(Self {
            every: shape.every,
            unit: shape.unit,
            anchor: shape.anchor,
            from,
        })
    }
}

/// Writes the first date of a chain as a date value does.
#[expect(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "the signature `serialize_with` asks for"
)]
fn date_text_of<S: serde::Serializer>(from: &Option<Date>, out: S) -> Result<S::Ok, S::Error> {
    match from {
        Some(date) => out.collect_str(date),
        None => out.serialize_none(),
    }
}

impl Recurrence {
    /// How the card whose fields these are repeats; `None` for a card that does not, or
    /// whose member is no recurrence.
    #[must_use]
    pub fn of_fields(fields: &JsonMap) -> Option<Self> {
        fields
            .get(RECURRENCE_FIELD)
            .and_then(|value| Self::deserialize(value).ok())
    }

    /// The recurrence as a card's fields hold it.
    #[must_use]
    pub fn to_value(self) -> Value {
        let mut value = json!({
            "every": self.every,
            "unit": self.unit,
            "anchor": self.anchor,
        });
        if let Some(from) = self.from {
            value["from"] = Value::from(from.to_string());
        }
        value
    }

    /// Whether `other` repeats as this does, wherever its chain is counted from.
    #[must_use]
    pub fn same_rule(self, other: Self) -> bool {
        (self.every, self.unit, self.anchor) == (other.every, other.unit, other.anchor)
    }

    /// The same rule, as the first card of a chain has it.
    #[must_use]
    pub const fn anew(self) -> Self {
        Self { from: None, ..self }
    }

    /// How the next card repeats, after a card due `due` (in `time_zone`'s days):
    /// counted from the due date, from the first date of the chain, which is `due`'s
    /// own for the first card of one; counted from the day it is done, from nothing.
    #[must_use]
    pub fn handed_on(self, due: Option<&Value>, time_zone: &TimeZone) -> Self {
        let from = match self.anchor {
            RecurrenceAnchor::Due => day_of(due, &zone_of(time_zone)).map(|old| self.origin(old)),
            RecurrenceAnchor::Done => None,
        };
        Self { from, ..self }
    }

    /// The date the steps of a card due `old` are counted from: its chain's first,
    /// unless that is after `old`, which counts for none.
    fn origin(self, old: Date) -> Date {
        self.from.filter(|from| *from <= old).unwrap_or(old)
    }

    /// `count` steps of the recurrence.
    fn periods(self, count: i64) -> Option<Span> {
        let units = i64::from(self.every).checked_mul(count)?;
        let span = Span::new();
        match self.unit {
            RecurrenceUnit::Day => span.try_days(units),
            RecurrenceUnit::Week => span.try_weeks(units),
            RecurrenceUnit::Month => span.try_months(units),
            RecurrenceUnit::Year => span.try_years(units),
        }
        .ok()
    }

    /// The first date after `today` a whole number of steps (one at least) after
    /// `anchor`, each counted from `anchor` itself: the 31st of January three months on
    /// is the 30th of April, and four the 31st of May.
    fn first_after(self, anchor: Date, today: Date) -> Option<Date> {
        let unit = match self.unit {
            RecurrenceUnit::Day | RecurrenceUnit::Week => Unit::Day,
            RecurrenceUnit::Month => Unit::Month,
            RecurrenceUnit::Year => Unit::Year,
        };
        let between = anchor.until((unit, today)).ok()?;
        let whole = match self.unit {
            RecurrenceUnit::Day => i64::from(between.get_days()),
            RecurrenceUnit::Week => i64::from(between.get_days()) / 7,
            RecurrenceUnit::Month => i64::from(between.get_months()),
            RecurrenceUnit::Year => i64::from(between.get_years()),
        };
        // One step short of the steps that fit, so that none after today is passed.
        let mut count = (whole / i64::from(self.every)).saturating_sub(1).max(1);
        for _ in 0..4 {
            let date = anchor.checked_add(self.periods(count)?).ok()?;
            if date > today {
                return Some(date);
            }
            count = count.checked_add(1)?;
        }
        None
    }

    /// The due value of the next card: of the kind the completed card's is (a date, a
    /// range of dates, a time), moved on as [`RecurrenceAnchor`] says; a range keeps its
    /// length, a time its time of day in `time_zone`. Today and the day of a time are
    /// those of `time_zone`, UTC where the settings leave the zone to the device.
    /// Counted from the due date, it is after the completed card's too, a whole number
    /// of steps after the first date of the chain ([`Recurrence::from`]). A card
    /// without a due date has its next one step after today. `None` past the years a
    /// date may have.
    #[must_use]
    pub fn next_due(
        self,
        due: Option<&Value>,
        now_millis: i64,
        time_zone: &TimeZone,
    ) -> Option<Value> {
        let zone = zone_of(time_zone);
        let today = zone
            .to_datetime(Timestamp::from_millisecond(now_millis).ok()?)
            .date();
        let next_date = |old: Date| match self.anchor {
            RecurrenceAnchor::Due => self.first_after(self.origin(old), today.max(old)),
            RecurrenceAnchor::Done => today.checked_add(self.periods(1)?).ok(),
        };
        let due = due.and_then(Value::as_object);
        let from = due.and_then(|value| value.get("from"));
        if let Some(day) = from.and_then(Value::as_str) {
            let old: Date = day.parse().ok()?;
            let new = next_date(old)?;
            let mut next = json!({"from": date_text(new)?});
            if let Some(to) = due
                .and_then(|value| value.get("to"))
                .and_then(Value::as_str)
            {
                let moved = to
                    .parse::<Date>()
                    .ok()?
                    .checked_add(old.until(new).ok()?)
                    .ok()?;
                next["to"] = Value::from(date_text(moved)?);
            }
            return Some(next);
        }
        if let Some(millis) = from.and_then(Value::as_i64) {
            let old = zone.to_datetime(Timestamp::from_millisecond(millis).ok()?);
            let new = next_date(old.date())?.to_datetime(old.time());
            let landing = zone.to_timestamp(new).ok()?.as_millisecond();
            let moved_by = landing.checked_sub(millis)?;
            let mut next = json!({"from": time_value(landing)?});
            if let Some(to) = due
                .and_then(|value| value.get("to"))
                .and_then(Value::as_i64)
            {
                next["to"] = Value::from(time_value(to.checked_add(moved_by)?)?);
            }
            return Some(next);
        }
        let first = today.checked_add(self.periods(1)?).ok()?;
        Some(json!({"from": date_text(first)?}))
    }
}

/// The zone of the settings' time zone: UTC where they leave it to the device.
fn zone_of(time_zone: &TimeZone) -> jiff::tz::TimeZone {
    time_zone
        .name()
        .and_then(|name| jiff::tz::TimeZoneDatabase::bundled().get(name).ok())
        .unwrap_or(jiff::tz::TimeZone::UTC)
}

/// The day a due value begins on in `zone`: its first date, or the day of its time.
fn day_of(due: Option<&Value>, zone: &jiff::tz::TimeZone) -> Option<Date> {
    let from = due.and_then(Value::as_object)?.get("from")?;
    if let Some(day) = from.as_str() {
        return day.parse().ok();
    }
    let millis = from.as_i64()?;
    Some(
        zone.to_datetime(Timestamp::from_millisecond(millis).ok()?)
            .date(),
    )
}

/// A date as a date value writes it, if it is one a date value may have.
fn date_text(date: Date) -> Option<String> {
    let text = date.to_string();
    is_calendar_date(&text).then_some(text)
}

/// A time as a date value writes it, if it is one a date value may have.
fn time_value(millis: i64) -> Option<i64> {
    (0..=LAST_MILLISECOND).contains(&millis).then_some(millis)
}

/// Whether `value` is how a card repeats, as a card's fields hold it.
pub(crate) fn is_recurrence_value(value: &Value) -> bool {
    Recurrence::deserialize(value).is_ok()
}

/// Whether `value` may name the card made after a completed one.
pub(crate) fn is_recurred_to_value(value: &Value) -> bool {
    value.as_str().is_some_and(crate::id::is_id)
}

/// The value of the property of the rule of done (`definition`) that the next card
/// starts with, not done, from the completed card's (`current`) and the one it had
/// before it was done (`before`): for a choice of one, the option it was in if that is
/// one of the property's and not done, else the first that is not done; for a choice
/// of several, what was chosen but the options that are done; for a checkbox, none.
/// `None` for no value.
#[must_use]
pub fn not_done_value(
    definition: &CardPropertyDefinition,
    done_when: &DoneWhen,
    current: Option<&Value>,
    before: Option<&Value>,
) -> Option<Value> {
    let is_done = |id: &str| done_when.option_ids.iter().any(|done| done == id);
    let open = |id: &str| !is_done(id) && definition.options.iter().any(|option| option.id == id);
    match definition.property_type {
        PropertyType::Select => before
            .and_then(Value::as_str)
            .filter(|id| open(id))
            .or_else(|| {
                definition
                    .options
                    .iter()
                    .map(|option| option.id.as_str())
                    .find(|id| !is_done(id))
            })
            .map(Value::from),
        PropertyType::MultiSelect => {
            let kept: Vec<Value> = current
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|item| item.as_str().is_some_and(|id| !is_done(id)))
                .cloned()
                .collect();
            (!kept.is_empty()).then_some(Value::Array(kept))
        }
        PropertyType::Checkbox => None,
        _ => current.cloned(),
    }
}

/// The body with every task item unchecked, for the next card of one that repeats.
#[must_use]
pub fn with_tasks_unchecked(body: &Body) -> Body {
    let mut document = body.document().clone();
    uncheck(&mut document);
    Body::from_stored(document)
}

fn uncheck(node: &mut Value) {
    let Value::Object(members) = node else {
        return;
    };
    if members.get("type").and_then(Value::as_str) == Some("taskItem")
        && let Some(Value::Object(attrs)) = members.get_mut("attrs")
        && attrs.contains_key("checked")
    {
        attrs.insert("checked".to_owned(), Value::Bool(false));
    }
    if let Some(Value::Array(content)) = members.get_mut("content") {
        content.iter_mut().for_each(uncheck);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card_property::{PropertyColor, PropertyOption};

    const fn every(every: u16, unit: RecurrenceUnit, anchor: RecurrenceAnchor) -> Recurrence {
        Recurrence {
            every,
            unit,
            anchor,
            from: None,
        }
    }

    /// Noon UTC of the day.
    fn noon_of(day: &str) -> i64 {
        let date: Date = day.parse().expect("a date");
        jiff::tz::TimeZone::UTC
            .to_timestamp(date.to_datetime(jiff::civil::time(12, 0, 0, 0)))
            .expect("a time")
            .as_millisecond()
    }

    fn utc() -> TimeZone {
        TimeZone::default()
    }

    fn next(recurrence: Recurrence, due: &Value, today: &str) -> Value {
        recurrence
            .next_due(Some(due), noon_of(today), &utc())
            .expect("a next due date")
    }

    #[test]
    fn a_due_date_moves_on_by_its_period() {
        use RecurrenceAnchor::Due;
        use RecurrenceUnit::{Day, Month, Week, Year};
        let due = json!({"from": "2026-03-05"});
        let on_time = "2026-03-05";
        assert_eq!(
            next(every(1, Day, Due), &due, on_time),
            json!({"from": "2026-03-06"})
        );
        assert_eq!(
            next(every(2, Week, Due), &due, on_time),
            json!({"from": "2026-03-19"})
        );
        assert_eq!(
            next(every(1, Month, Due), &due, on_time),
            json!({"from": "2026-04-05"})
        );
        assert_eq!(
            next(every(1, Year, Due), &due, on_time),
            json!({"from": "2027-03-05"})
        );
        // Completed early: one period after the due date all the same.
        assert_eq!(
            next(every(1, Week, Due), &due, "2026-03-01"),
            json!({"from": "2026-03-12"})
        );
    }

    #[test]
    fn the_end_of_a_month_is_the_end_of_a_shorter_one_and_comes_back() {
        use RecurrenceAnchor::Due;
        use RecurrenceUnit::Month;
        let due = json!({"from": "2026-01-31"});
        let monthly = every(1, Month, Due);
        assert_eq!(
            next(monthly, &due, "2026-01-31"),
            json!({"from": "2026-02-28"})
        );
        // Counted from the anchor, not from the shorter month: the 31st again.
        assert_eq!(
            next(monthly, &due, "2026-02-28"),
            json!({"from": "2026-03-31"})
        );
        assert_eq!(
            next(monthly, &due, "2026-04-29"),
            json!({"from": "2026-04-30"})
        );
    }

    /// The due dates of the `count` cards after one due `first`, each completed
    /// `early` days before it is due.
    fn chain(recurrence: Recurrence, first: &str, count: usize, early: i64) -> Vec<String> {
        let (mut recurrence, mut due) = (recurrence, json!({"from": first}));
        let mut dues = Vec::new();
        for _ in 0..count {
            let day: Date = due["from"]
                .as_str()
                .expect("a date")
                .parse()
                .expect("a date");
            let today = day.checked_sub(Span::new().days(early)).expect("a day");
            let following = recurrence.handed_on(Some(&due), &utc());
            due = next(recurrence, &due, &today.to_string());
            recurrence = following;
            dues.push(due["from"].as_str().expect("a date").to_owned());
        }
        dues
    }

    #[test]
    fn a_chain_keeps_the_end_of_the_month_and_the_29th_of_february() {
        use RecurrenceAnchor::Due;
        use RecurrenceUnit::{Month, Year};
        for early in [0, 3] {
            assert_eq!(
                chain(every(1, Month, Due), "2026-01-31", 4, early),
                ["2026-02-28", "2026-03-31", "2026-04-30", "2026-05-31"],
                "completed {early} days early"
            );
            assert_eq!(
                chain(every(1, Year, Due), "2024-02-29", 4, early),
                ["2025-02-28", "2026-02-28", "2027-02-28", "2028-02-29"],
                "completed {early} days early"
            );
        }
        let monthly = every(1, Month, Due);
        let second = monthly.handed_on(Some(&json!({"from": "2026-01-31"})), &utc());
        assert_eq!(second.from, Some(jiff::civil::date(2026, 1, 31)));
        assert_eq!(
            second.handed_on(Some(&json!({"from": "2026-02-28"})), &utc()),
            second,
            "the first date of the chain stays"
        );
        assert_eq!(
            every(1, Month, RecurrenceAnchor::Done)
                .handed_on(Some(&json!({"from": "2026-01-31"})), &utc())
                .from,
            None,
            "counted from the day it is done, a chain counts from nothing"
        );
        // A first date after the due date counts for none: the due date's own is.
        let ahead = Recurrence {
            from: Some(jiff::civil::date(2026, 6, 30)),
            ..monthly
        };
        let due = json!({"from": "2026-01-31"});
        assert_eq!(
            next(ahead, &due, "2026-01-31"),
            json!({"from": "2026-02-28"})
        );
        assert_eq!(ahead.handed_on(Some(&due), &utc()), second);
    }

    #[test]
    fn the_29th_of_february_is_the_28th_of_a_common_year() {
        use RecurrenceAnchor::Due;
        use RecurrenceUnit::Year;
        let due = json!({"from": "2024-02-29"});
        assert_eq!(
            next(every(1, Year, Due), &due, "2024-02-29"),
            json!({"from": "2025-02-28"})
        );
        assert_eq!(
            next(every(4, Year, Due), &due, "2024-02-29"),
            json!({"from": "2028-02-29"})
        );
    }

    #[test]
    fn a_card_completed_late_keeps_its_days_and_lands_after_today() {
        use RecurrenceAnchor::{Done, Due};
        use RecurrenceUnit::{Day, Month, Week};
        let due = json!({"from": "2026-03-02"});
        // Due on Mondays, completed on the Thursday of the week after.
        assert_eq!(
            next(every(1, Week, Due), &due, "2026-03-12"),
            json!({"from": "2026-03-16"})
        );
        // Due today: the next is not today.
        assert_eq!(
            next(every(1, Week, Due), &due, "2026-03-09"),
            json!({"from": "2026-03-16"})
        );
        // Years behind, every three days: the first after today on the old rhythm.
        let old = json!({"from": "2020-01-01"});
        let moved = next(every(3, Day, Due), &old, "2026-03-05");
        let moved: Date = moved["from"]
            .as_str()
            .expect("a date")
            .parse()
            .expect("a date");
        let start: Date = "2020-01-01".parse().expect("a date");
        let today: Date = "2026-03-05".parse().expect("a date");
        assert!(moved > today && moved <= today.checked_add(Span::new().days(3)).expect("a day"));
        assert_eq!(
            start.until((Unit::Day, moved)).expect("days").get_days() % 3,
            0
        );
        assert_eq!(
            next(every(1, Month, Due), &old, "2026-03-05"),
            json!({"from": "2026-04-01"})
        );
        // From the day it was done.
        assert_eq!(
            next(every(10, Day, Done), &due, "2026-03-12"),
            json!({"from": "2026-03-22"})
        );
    }

    #[test]
    fn a_range_keeps_its_length_and_a_time_its_hour() {
        use RecurrenceAnchor::Due;
        use RecurrenceUnit::{Month, Week};
        let range = json!({"from": "2026-03-30", "to": "2026-04-02"});
        assert_eq!(
            next(every(1, Week, Due), &range, "2026-03-30"),
            json!({"from": "2026-04-06", "to": "2026-04-09"})
        );
        let seoul: TimeZone = "Asia/Seoul".parse().expect("a zone");
        // 09:00 in Seoul on the 31st of January is 00:00 UTC.
        let nine: i64 = 1_769_817_600_000;
        let timed = json!({"from": nine, "to": nine + 3_600_000});
        let moved = every(1, Month, Due)
            .next_due(Some(&timed), nine, &seoul)
            .expect("a next due time");
        // 09:00 in Seoul on the 28th of February.
        let february: i64 = 1_772_236_800_000;
        assert_eq!(moved, json!({"from": february, "to": february + 3_600_000}));
    }

    #[test]
    fn today_is_the_completers_day() {
        use RecurrenceAnchor::Done;
        use RecurrenceUnit::Day;
        // 23:30 UTC on the 1st of March is the 2nd in Seoul and the 1st in Los Angeles.
        let late = noon_of("2026-03-01") + 11 * 3_600_000 + 30 * 60_000;
        let seoul: TimeZone = "Asia/Seoul".parse().expect("a zone");
        let los_angeles: TimeZone = "America/Los_Angeles".parse().expect("a zone");
        let daily = every(1, Day, Done);
        let due = json!({"from": "2026-02-20"});
        assert_eq!(
            daily.next_due(Some(&due), late, &seoul),
            Some(json!({"from": "2026-03-03"}))
        );
        assert_eq!(
            daily.next_due(Some(&due), late, &los_angeles),
            Some(json!({"from": "2026-03-02"}))
        );
        // The device's zone, unknown to the server, is UTC.
        assert_eq!(
            daily.next_due(Some(&due), late, &utc()),
            Some(json!({"from": "2026-03-02"}))
        );
    }

    #[test]
    fn a_card_without_a_due_date_is_due_one_step_after_today() {
        let weekly = every(1, RecurrenceUnit::Week, RecurrenceAnchor::Due);
        assert_eq!(
            weekly.next_due(None, noon_of("2026-03-05"), &utc()),
            Some(json!({"from": "2026-03-12"}))
        );
    }

    #[test]
    fn a_recurrence_is_read_within_its_range() {
        let read = |value: Value| Recurrence::deserialize(value).ok();
        assert_eq!(
            read(json!({"every": 2, "unit": "week", "anchor": "done"})),
            Some(every(2, RecurrenceUnit::Week, RecurrenceAnchor::Done))
        );
        for refused in [
            json!({"every": 0, "unit": "day", "anchor": "due"}),
            json!({"every": 366, "unit": "day", "anchor": "due"}),
            json!({"every": 1, "unit": "hour", "anchor": "due"}),
            json!({"every": 1, "unit": "day", "anchor": "due", "more": 1}),
            json!("weekly"),
        ] {
            assert_eq!(read(refused.clone()), None, "{refused}");
        }
        let weekly = every(1, RecurrenceUnit::Week, RecurrenceAnchor::Due);
        assert_eq!(read(weekly.to_value()), Some(weekly));
        let chained = Recurrence {
            from: Some(jiff::civil::date(2026, 1, 31)),
            ..weekly
        };
        let written = json!({"every": 1, "unit": "week", "anchor": "due", "from": "2026-01-31"});
        assert_eq!(chained.to_value(), written);
        assert_eq!(serde_json::to_value(chained).ok(), Some(written.clone()));
        assert_eq!(read(written), Some(chained));
        assert_eq!(serde_json::to_value(weekly).ok(), Some(weekly.to_value()));
        for refused in ["2026-02-30", "0000-01-01", "2026-1-31", "yesterday"] {
            let value = json!({"every": 1, "unit": "week", "anchor": "due", "from": refused});
            assert_eq!(read(value), None, "{refused}");
        }
        assert!(
            chained.same_rule(weekly)
                && !chained.same_rule(every(2, RecurrenceUnit::Week, RecurrenceAnchor::Due))
        );
        assert_eq!(chained.anew(), weekly);
    }

    #[test]
    fn the_next_card_starts_not_done() {
        let (todo, doing, done) = ("o-todo", "o-doing", "o-done");
        let status = CardPropertyDefinition {
            id: "status".to_owned(),
            name: "Status".to_owned(),
            property_type: PropertyType::Select,
            options: vec![
                PropertyOption::new(done.to_owned(), "Done", PropertyColor::Green),
                PropertyOption::new(todo.to_owned(), "Todo", PropertyColor::Gray),
                PropertyOption::new(doing.to_owned(), "Doing", PropertyColor::Blue),
            ],
        };
        let rule = DoneWhen {
            property_id: "status".to_owned(),
            option_ids: vec![done.to_owned()],
        };
        assert_eq!(
            not_done_value(&status, &rule, Some(&json!(done)), None),
            Some(json!(todo)),
            "the first option that is not done"
        );
        assert_eq!(
            not_done_value(&status, &rule, Some(&json!(done)), Some(&json!(doing))),
            Some(json!(doing)),
            "the option it was in before it was done"
        );
        for gone in [json!("o-gone"), json!(done), json!(7)] {
            assert_eq!(
                not_done_value(&status, &rule, Some(&json!(done)), Some(&gone)),
                Some(json!(todo)),
                "{gone} is no option to go back to"
            );
        }
        let tags = CardPropertyDefinition {
            property_type: PropertyType::MultiSelect,
            ..status
        };
        assert_eq!(
            not_done_value(&tags, &rule, Some(&json!([doing, done])), None),
            Some(json!([doing]))
        );
        assert_eq!(
            not_done_value(&tags, &rule, Some(&json!([done])), None),
            None
        );
        let check = CardPropertyDefinition::new("check", "Done", PropertyType::Checkbox);
        assert_eq!(
            not_done_value(&check, &rule, Some(&json!("true")), None),
            None
        );
    }

    #[test]
    fn the_task_items_of_the_next_body_are_unchecked() {
        let item = |checked: bool| {
            json!({"type": "taskItem", "attrs": {"checked": checked}, "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "t"}]}
            ]})
        };
        let body = Body::from_stored(json!({"type": "doc", "content": [
            {"type": "taskList", "content": [item(true), item(false), {"type": "taskItem", "attrs": {"checked": true}, "content": [
                {"type": "taskList", "content": [item(true)]}
            ]}]}
        ]}));
        let unchecked = with_tasks_unchecked(&body);
        assert_eq!(body.tasks().done, 3);
        assert_eq!(unchecked.tasks().total, 4);
        assert_eq!(unchecked.tasks().done, 0);
    }
}
