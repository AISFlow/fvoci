//! The value of a date property.
//!
//! A calendar date is a `YYYY-MM-DD` string, the same day for everybody, and a time is
//! an integer of milliseconds since 1970-01-01 00:00 UTC:
//!
//! | Value | Shape |
//! |---|---|
//! | A date | `{"from": "2026-03-05"}` |
//! | A range of dates | `{"from": "2026-03-05", "to": "2026-03-09"}`; `to` is the last day |
//! | A time | `{"from": 1772703000000}` |
//!
//! `to` is of the kind of `from`, not before it, and left out when it would equal it.
//! There are no other members: whether a time is set is said by the kind, and the time
//! zone is the viewer's (an account setting), not the value's.

use std::collections::HashSet;
use std::hash::BuildHasher;

use serde_json::Value;
use thiserror::Error;

use crate::JsonMap;
use crate::card_property::{CardPropertyDefinition, PropertyType};
use crate::settings::TimeZone;

/// The last millisecond a time may be: 9999-12-31 23:59:59.999 UTC.
pub(crate) const LAST_MILLISECOND: i64 = 253_402_300_799_999;

/// Milliseconds in a day.
const DAY_MILLIS: i64 = 86_400_000;

/// Why a date value is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DateValueError {
    /// A member other than `from` and `to`.
    #[error("a date value has no members but from and to")]
    UnknownMember,
    /// No `from`.
    #[error("a date value needs from")]
    MissingFrom,
    /// A `from` or `to` that is neither a calendar date nor a time.
    #[error(
        "from and to are calendar dates (YYYY-MM-DD, years 1-9999) or milliseconds from 0 to the end of 9999"
    )]
    NotADateOrTime,
    /// `from` and `to` of different kinds.
    #[error("from and to are both dates or both times")]
    MixedKinds,
    /// `to` before `from`.
    #[error("to is not before from")]
    ToBeforeFrom,
    /// `to` the same as `from`.
    #[error("to is left out when it would be from")]
    ToIsFrom,
}

/// What a `from` or `to` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Point<'v> {
    Date(&'v str),
    Time(i64),
}

impl<'v> Point<'v> {
    fn read(value: &'v Value) -> Result<Self, DateValueError> {
        match value {
            Value::String(date) if is_calendar_date(date) => Ok(Self::Date(date)),
            Value::Number(number) => number
                .as_i64()
                .filter(|millis| (0..=LAST_MILLISECOND).contains(millis))
                .map(Self::Time)
                .ok_or(DateValueError::NotADateOrTime),
            _ => Err(DateValueError::NotADateOrTime),
        }
    }
}

/// Checks a date value.
///
/// # Errors
///
/// Returns the first [`DateValueError`] the value has.
pub fn check_date_value(value: &JsonMap) -> Result<(), DateValueError> {
    if value.keys().any(|key| key != "from" && key != "to") {
        return Err(DateValueError::UnknownMember);
    }
    let from = Point::read(value.get("from").ok_or(DateValueError::MissingFrom)?)?;
    let Some(to) = value.get("to") else {
        return Ok(());
    };
    // Dates in `YYYY-MM-DD` compare as their strings do.
    match (from, Point::read(to)?) {
        (Point::Date(_), Point::Time(_)) | (Point::Time(_), Point::Date(_)) => {
            Err(DateValueError::MixedKinds)
        }
        (from, to) if to < from => Err(DateValueError::ToBeforeFrom),
        (from, to) if to == from => Err(DateValueError::ToIsFrom),
        _ => Ok(()),
    }
}

/// Whether the text is a day of the calendar as `YYYY-MM-DD`, years 1 to 9999.
#[must_use]
pub fn is_calendar_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let number = |range: std::ops::Range<usize>| {
        bytes[range].iter().try_fold(0_u32, |sum, byte| {
            byte.is_ascii_digit()
                .then(|| sum * 10 + u32::from(byte - b'0'))
        })
    };
    let (Some(year), Some(month), Some(day)) = (number(0..4), number(5..7), number(8..10)) else {
        return false;
    };
    year >= 1 && (1..=12).contains(&month) && day >= 1 && day <= days_in_month(year, month)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The UTC calendar day of a time, as `YYYY-MM-DD`; `None` outside years 1 to 9999.
#[must_use]
pub fn utc_date(millis: i64) -> Option<String> {
    // Days to the civil date: Howard Hinnant's `civil_from_days`.
    let days = millis.div_euclid(DAY_MILLIS) + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (1..=9999)
        .contains(&year)
        .then(|| format!("{year:04}-{month:02}-{day:02}"))
}

/// The calendar day it is at a time for somebody whose settings name `time_zone`, as
/// `YYYY-MM-DD`. Where the settings leave the zone to the device, which the server does
/// not know, or name one the bundled database has not, it is the day in UTC. `None`
/// for a time outside years 1 to 9999.
#[must_use]
pub fn day_at(millis: i64, time_zone: &TimeZone) -> Option<String> {
    let zone = time_zone
        .name()
        .and_then(|name| jiff::tz::TimeZoneDatabase::bundled().get(name).ok())
        .unwrap_or(jiff::tz::TimeZone::UTC);
    let time = jiff::Timestamp::from_millisecond(millis).ok()?;
    let day = zone.to_datetime(time).date().to_string();
    is_calendar_date(&day).then_some(day)
}

/// How many days after the calendar day `from` the calendar day `to` is: negative if it
/// is before. `None` unless both are days as `YYYY-MM-DD`.
#[must_use]
pub fn days_between(from: &str, to: &str) -> Option<i64> {
    let from = calendar_date(from)?;
    let to = calendar_date(to)?;
    Some(i64::from(from.until(to).ok()?.get_days()))
}

/// Moves the calendar day `date` (`YYYY-MM-DD`) by `days`, returning the resulting
/// `YYYY-MM-DD` date or `None` if `date` is invalid or the result leaves years 1 to 9999.
#[must_use]
pub fn add_days(date: &str, days: i64) -> Option<String> {
    let span = jiff::Span::new().try_days(days).ok()?;
    let moved = calendar_date(date)?.checked_add(span).ok()?.to_string();
    is_calendar_date(&moved).then_some(moved)
}

/// Moves the days among the fields of a card or a view by `days`, given the
/// identifiers of the board's date properties: the date values of a card and the days a
/// view filters a date property by. A calendar date becomes the day that many days
/// later and a time is moved by that many times 24 hours; earlier for a negative
/// number. A value that is no date value, or would leave years 1 to 9999, stays as it
/// is.
pub fn move_dates<S: BuildHasher>(fields: &mut JsonMap, dates: &HashSet<String, S>, days: i64) {
    if days == 0 || dates.is_empty() {
        return;
    }
    if let Some(Value::Object(values)) = fields.get_mut("properties") {
        for (id, value) in values.iter_mut() {
            let Value::Object(value) = value else {
                continue;
            };
            if !dates.contains(id) || check_date_value(value).is_err() {
                continue;
            }
            let moved: Option<JsonMap> = value
                .iter()
                .map(|(member, point)| Some((member.clone(), moved_point(point, days)?)))
                .collect();
            if let Some(moved) = moved {
                *value = moved;
            }
        }
    }
    if let Some(filter) = fields.get_mut("filter") {
        move_filter(filter, dates, days);
    }
}

/// Moves the days that a view's filter group, and the groups within it, filter date
/// properties by.
fn move_filter<S: BuildHasher>(group: &mut Value, dates: &HashSet<String, S>, days: i64) {
    let Some(filters) = group.get_mut("filters").and_then(Value::as_array_mut) else {
        return;
    };
    for filter in filters {
        move_filter(filter, dates, days);
        let Some(filter) = filter.as_object_mut() else {
            continue;
        };
        if !is_date_filter(filter, dates) {
            continue;
        }
        let Some(values) = filter.get_mut("values").and_then(Value::as_array_mut) else {
            continue;
        };
        for value in values {
            if let Some(moved) = moved_point(value, days).filter(Value::is_string) {
                *value = moved;
            }
        }
    }
}

/// A `from` or `to` moved by `days`, or `None` if it would leave years 1 to 9999.
fn moved_point(point: &Value, days: i64) -> Option<Value> {
    match Point::read(point).ok()? {
        Point::Date(date) => {
            let span = jiff::Span::new().try_days(days).ok()?;
            let moved = calendar_date(date)?.checked_add(span).ok()?.to_string();
            is_calendar_date(&moved).then_some(Value::String(moved))
        }
        Point::Time(millis) => days
            .checked_mul(DAY_MILLIS)
            .and_then(|by| millis.checked_add(by))
            .filter(|moved| (0..=LAST_MILLISECOND).contains(moved))
            .map(Value::from),
    }
}

/// The day of the calendar that `text` names as `YYYY-MM-DD`, years 1 to 9999.
fn calendar_date(text: &str) -> Option<jiff::civil::Date> {
    is_calendar_date(text).then(|| text.parse().ok()).flatten()
}

/// The identifiers of the date properties among a board's definitions.
#[must_use]
pub fn date_property_ids<'d>(
    definitions: impl IntoIterator<Item = &'d CardPropertyDefinition>,
) -> HashSet<String> {
    definitions
        .into_iter()
        .filter(|definition| {
            !definition.id.is_empty() && definition.property_type == PropertyType::Date
        })
        .map(|definition| definition.id.clone())
        .collect()
}

/// Whether a filter of a view compares a date property.
fn is_date_filter<S: BuildHasher>(filter: &JsonMap, dates: &HashSet<String, S>) -> bool {
    filter
        .get("propertyId")
        .and_then(Value::as_str)
        .is_some_and(|id| dates.contains(id))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn check(value: &Value) -> Result<(), DateValueError> {
        check_date_value(value.as_object().expect("an object"))
    }

    #[test]
    fn dates_ranges_and_times_are_date_values() {
        for value in [
            json!({"from": "2026-03-05"}),
            json!({"from": "2026-03-05", "to": "2026-03-09"}),
            json!({"from": "2024-02-29"}),
            json!({"from": "0001-01-01", "to": "9999-12-31"}),
            json!({"from": 1_772_703_000_000_i64}),
            json!({"from": 0, "to": LAST_MILLISECOND}),
        ] {
            assert_eq!(check(&value), Ok(()), "{value}");
        }
    }

    #[test]
    fn what_is_no_date_value_is_refused_with_its_reason() {
        for (value, error) in [
            (json!({}), DateValueError::MissingFrom),
            (json!({"to": "2026-03-05"}), DateValueError::MissingFrom),
            (
                json!({"from": "2026-03-05", "includeTime": false}),
                DateValueError::UnknownMember,
            ),
            (
                json!({"from": "2026-02-30"}),
                DateValueError::NotADateOrTime,
            ),
            (
                json!({"from": "2023-02-29"}),
                DateValueError::NotADateOrTime,
            ),
            (
                json!({"from": "0000-01-01"}),
                DateValueError::NotADateOrTime,
            ),
            (json!({"from": "2026-3-05"}), DateValueError::NotADateOrTime),
            (
                json!({"from": "2026-03-05T00:00"}),
                DateValueError::NotADateOrTime,
            ),
            (json!({"from": -1}), DateValueError::NotADateOrTime),
            (
                json!({"from": LAST_MILLISECOND + 1}),
                DateValueError::NotADateOrTime,
            ),
            (json!({"from": 1.5}), DateValueError::NotADateOrTime),
            (
                json!({"from": "1772703000000"}),
                DateValueError::NotADateOrTime,
            ),
            (json!({"from": null}), DateValueError::NotADateOrTime),
            (
                json!({"from": "2026-03-05", "to": 1}),
                DateValueError::MixedKinds,
            ),
            (
                json!({"from": "2026-03-05", "to": "2026-03-04"}),
                DateValueError::ToBeforeFrom,
            ),
            (json!({"from": 5, "to": 4}), DateValueError::ToBeforeFrom),
            (
                json!({"from": "2026-03-05", "to": "2026-03-05"}),
                DateValueError::ToIsFrom,
            ),
        ] {
            assert_eq!(check(&value), Err(error), "{value}");
        }
    }

    #[test]
    fn utc_dates_are_the_days_of_the_calendar() {
        assert_eq!(utc_date(0).as_deref(), Some("1970-01-01"));
        assert_eq!(utc_date(-1).as_deref(), Some("1969-12-31"));
        assert_eq!(utc_date(951_782_400_000).as_deref(), Some("2000-02-29"));
        assert_eq!(utc_date(LAST_MILLISECOND).as_deref(), Some("9999-12-31"));
        assert_eq!(utc_date(LAST_MILLISECOND + 1), None, "year 10000");
        assert_eq!(utc_date(-62_135_596_800_001), None, "year 0");
        assert_eq!(utc_date(-62_135_596_800_000).as_deref(), Some("0001-01-01"));
    }

    #[test]
    fn the_day_is_that_of_the_named_zone_or_else_of_utc() {
        // 2026-12-31 20:00 UTC is the next year's first morning in Seoul.
        let millis = 1_798_747_200_000;
        let zone = |name: &str| name.parse::<TimeZone>().expect(name);
        assert_eq!(day_at(millis, &zone("auto")).as_deref(), Some("2026-12-31"));
        assert_eq!(day_at(millis, &zone("UTC")).as_deref(), Some("2026-12-31"));
        assert_eq!(
            day_at(millis, &zone("Asia/Seoul")).as_deref(),
            Some("2027-01-01")
        );
        assert_eq!(
            day_at(millis, &zone("America/Los_Angeles")).as_deref(),
            Some("2026-12-31")
        );
        assert_eq!(day_at(LAST_MILLISECOND, &zone("Asia/Seoul")), None);
        assert_eq!(day_at(i64::MAX, &zone("auto")), None);
    }

    #[test]
    fn days_between_counts_the_days_of_the_calendar() {
        assert_eq!(days_between("2026-12-01", "2026-12-01"), Some(0));
        assert_eq!(days_between("2026-12-01", "2027-03-01"), Some(90));
        assert_eq!(
            days_between("2028-02-28", "2028-03-01"),
            Some(2),
            "a leap day"
        );
        assert_eq!(days_between("2026-12-01", "2026-11-19"), Some(-12));
        assert_eq!(days_between("0001-01-01", "9999-12-31"), Some(3_652_058));
        assert_eq!(days_between("2026-12-01", "tomorrow"), None);
        assert_eq!(days_between("2026-02-30", "2026-03-01"), None);
    }

    fn fields(value: &Value) -> JsonMap {
        value.as_object().expect("an object").clone()
    }

    #[test]
    fn date_values_move_by_days_and_nothing_else_does() {
        let dates: HashSet<String> = ["due", "met", "odd"].map(str::to_owned).into();
        let before = json!({"properties": {
            "due": {"from": "2026-12-30", "to": "2027-01-02"},
            "met": {"from": 1_772_703_000_000_i64},
            "odd": "2026-12-30",
            "note": {"from": "2026-12-30"},
        }});
        let mut card = fields(&before);
        move_dates(&mut card, &dates, 60);
        assert_eq!(
            Value::Object(card.clone()),
            json!({"properties": {
                "due": {"from": "2027-02-28", "to": "2027-03-03"},
                "met": {"from": 1_772_703_000_000_i64 + 60 * DAY_MILLIS},
                "odd": "2026-12-30",
                "note": {"from": "2026-12-30"},
            }})
        );
        move_dates(&mut card, &dates, -60);
        assert_eq!(Value::Object(card), before);
    }

    #[test]
    fn the_days_a_view_filters_dates_by_move_with_them() {
        let dates: HashSet<String> = ["due"].map(str::to_owned).into();
        let mut view = fields(&json!({"filter": {"operation": "and", "filters": [
            {"propertyId": "due", "condition": "isBefore", "values": ["2026-12-30"]},
            {"propertyId": "note", "condition": "is", "values": ["2026-12-30"]},
            {"operation": "or", "filters": [
                {"propertyId": "due", "condition": "isSet", "values": []},
                {"propertyId": "due", "condition": "is", "values": ["2026-12-01", "soon"]},
            ]},
        ]}}));
        move_dates(&mut view, &dates, 3);
        assert_eq!(
            Value::Object(view),
            json!({"filter": {"operation": "and", "filters": [
                {"propertyId": "due", "condition": "isBefore", "values": ["2027-01-02"]},
                {"propertyId": "note", "condition": "is", "values": ["2026-12-30"]},
                {"operation": "or", "filters": [
                    {"propertyId": "due", "condition": "isSet", "values": []},
                    {"propertyId": "due", "condition": "is", "values": ["2026-12-04", "soon"]},
                ]},
            ]}})
        );
    }

    #[test]
    fn a_date_value_that_would_leave_the_calendar_stays() {
        let dates: HashSet<String> = ["due", "met"].map(str::to_owned).into();
        let last = json!({"properties": {"due": {"from": "9999-12-01", "to": "9999-12-31"}}});
        let first = json!({"properties": {
            "due": {"from": "0001-01-01"},
            "met": {"from": 5},
        }});
        for (before, days, what) in [
            (&last, 1, "the end would be past 9999"),
            (&last, i64::MAX, "more days than there are"),
            (&first, -1, "before year 1, and before 1970"),
            (&first, i64::MIN, "more days than there are"),
        ] {
            let mut card = fields(before);
            move_dates(&mut card, &dates, days);
            assert_eq!(&Value::Object(card), before, "{what}");
        }
    }
}
