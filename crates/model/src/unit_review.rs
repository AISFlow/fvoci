//! Personal spaced-repetition reviews (`unit_reviews`) and personal study notes
//! (`unit_notes`) on course units.
//!
//! Every review schedule and note belongs strictly to its learner and is never disclosed
//! to other course members, workspace administrators, organization administrators, or
//! public-link readers.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::JsonMap;
use crate::date_value::{add_days, days_between, is_calendar_date};
use crate::refusal::{RefusalReason, Refusals};

/// Default spaced-repetition interval sequence in days (`[1, 3, 7, 14, 30]`).
pub const DEFAULT_REVIEW_INTERVALS: [u32; 5] = [1, 3, 7, 14, 30];

/// Minimum review interval in days.
pub const MIN_REVIEW_INTERVAL_DAYS: u32 = 1;

/// Maximum review interval in days (roughly ten years).
pub const MAX_REVIEW_INTERVAL_DAYS: u32 = 3650;

/// Maximum length in Unicode scalar values of [`UnitNote::content`].
pub const UNIT_NOTE_CONTENT_MAX_CHARS: usize = 10_000;

/// Course container property key holding an optional custom review interval sequence.
pub const COURSE_REVIEW_INTERVALS_PROPERTY: &str = "defaultReviewIntervals";

const INTERVAL_OUT_OF_RANGE: &str = "must be between 1 and 3650 days";
const SCHEDULE_REQUIRED: &str = "intervalDays or scheduledDate is required";
const INVALID_DATE: &str = "must be a calendar date (YYYY-MM-DD)";
const DATE_OUT_OF_RANGE: &str = "the resulting date is outside years 1 to 9999";
const CONTENT_EMPTY: &str = "must not be empty";
const CONTENT_TOO_LONG: &str = "at most 10000 characters";
const CONTENT_CONTROL_CHARS: &str =
    "control characters other than line breaks and tabs are not allowed";

/// Returns the sorted, deduplicated review interval sequence configured on a course's
/// `properties` under `defaultReviewIntervals`, falling back to
/// [`DEFAULT_REVIEW_INTERVALS`] when absent or invalid.
#[must_use]
pub fn course_review_intervals(properties: &JsonMap) -> Vec<u32> {
    let custom = properties
        .get(COURSE_REVIEW_INTERVALS_PROPERTY)
        .and_then(Value::as_array)
        .map(|items| {
            let mut intervals: Vec<u32> = items
                .iter()
                .filter_map(Value::as_u64)
                .filter_map(|value| u32::try_from(value).ok())
                .filter(|&days| {
                    (MIN_REVIEW_INTERVAL_DAYS..=MAX_REVIEW_INTERVAL_DAYS).contains(&days)
                })
                .collect();
            intervals.sort_unstable();
            intervals.dedup();
            intervals
        });
    match custom {
        Some(intervals) if !intervals.is_empty() => intervals,
        _ => DEFAULT_REVIEW_INTERVALS.to_vec(),
    }
}

/// Computes the suggested next review interval in days given the learner's `current`
/// interval (or `None` on initial unit completion) and the course's interval sequence
/// (in any order).
///
/// With the default sequence `[1, 3, 7, 14, 30]`:
/// - `None` → `1`
/// - `Some(1)` → `3`
/// - `Some(3)` → `7`
/// - `Some(7)` → `14`
/// - `Some(14)` → `30`
/// - `Some(30)` or larger → `30`
#[must_use]
pub fn suggested_next_interval_days(current: Option<u32>, intervals: &[u32]) -> u32 {
    let sequence = if intervals.is_empty() {
        DEFAULT_REVIEW_INTERVALS.as_slice()
    } else {
        intervals
    };
    let Some(previous) = current else {
        return sequence
            .iter()
            .copied()
            .min()
            .unwrap_or(DEFAULT_REVIEW_INTERVALS[0]);
    };
    sequence
        .iter()
        .copied()
        .filter(|&candidate| candidate > previous)
        .min()
        .or_else(|| sequence.iter().copied().max())
        .unwrap_or(DEFAULT_REVIEW_INTERVALS[DEFAULT_REVIEW_INTERVALS.len() - 1])
}

/// Filter for listing a learner's course reviews by completion state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewStatusFilter {
    /// Both pending and completed reviews.
    #[default]
    All,
    /// Only reviews whose `completed_at` is `None`.
    Pending,
    /// Only reviews whose `completed_at` is `Some`.
    Completed,
}

impl ReviewStatusFilter {
    /// Returns the query-parameter word for this filter.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Pending => "pending",
            Self::Completed => "completed",
        }
    }
}

impl fmt::Display for ReviewStatusFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a review status filter word is not recognized.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("status must be one of pending, completed, or all (got {0:?})")]
pub struct ParseReviewStatusError(pub String);

impl FromStr for ReviewStatusFilter {
    type Err = ParseReviewStatusError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "all" => Ok(Self::All),
            "pending" => Ok(Self::Pending),
            "completed" => Ok(Self::Completed),
            other => Err(ParseReviewStatusError(other.to_owned())),
        }
    }
}

/// One personal spaced-repetition review scheduled for a unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnitReview {
    /// Unique identifier of this review.
    pub id: String,
    /// Identifier of the unit block.
    pub unit_id: String,
    /// Identifier of the course container.
    pub container_id: String,
    /// Scheduled calendar date (`YYYY-MM-DD`).
    pub scheduled_date: String,
    /// Completion time in milliseconds since the Unix epoch; `null` while pending.
    pub completed_at: Option<i64>,
    /// Review interval in days (`1..=3650`).
    pub interval_days: u32,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Last modification time in milliseconds since the Unix epoch.
    pub updated_at: i64,
}

/// Deserializes an explicit `u32` while refusing `null` (omitted fields remain `None`).
fn deserialize_non_null_u32<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<u32>, D::Error> {
    u32::deserialize(deserializer).map(Some)
}

/// Deserializes an explicit `String` while refusing `null` (omitted fields remain `None`).
fn deserialize_non_null_string<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

/// Deserializes an explicit [`UnitNoteKind`] while refusing `null`.
fn deserialize_non_null_note_kind<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<UnitNoteKind>, D::Error> {
    UnitNoteKind::deserialize(deserializer).map(Some)
}

/// Request body for scheduling a personal review on a course unit.
///
/// Provide `intervalDays` (`1..=3650`), `scheduledDate` (`YYYY-MM-DD`), or both. When
/// only `intervalDays` is sent, `scheduledDate` is computed as `today + intervalDays` in
/// the learner's configured time zone (or UTC). When only `scheduledDate` is sent,
/// `intervalDays` is derived from the number of days from `today` clamped to `1..=3650`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleUnitReview {
    /// Review interval in days (`1..=3650`).
    #[serde(default, deserialize_with = "deserialize_non_null_u32")]
    #[schema(value_type = u32, minimum = 1, maximum = 3650)]
    pub interval_days: Option<u32>,
    /// Target calendar date (`YYYY-MM-DD`).
    #[serde(default, deserialize_with = "deserialize_non_null_string")]
    #[schema(value_type = String)]
    pub scheduled_date: Option<String>,
}

/// Validated review schedule ready for storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUnitReview {
    /// Scheduled calendar date (`YYYY-MM-DD`).
    pub scheduled_date: String,
    /// Review interval in days (`1..=3650`).
    pub interval_days: u32,
}

impl ScheduleUnitReview {
    /// Validates the request and resolves `(scheduled_date, interval_days)` against the
    /// learner's current calendar day `today` (`YYYY-MM-DD`).
    ///
    /// # Errors
    ///
    /// Returns [`Refusals`] if neither field is set, `intervalDays` is outside
    /// `1..=3650`, `scheduledDate` is not a valid `YYYY-MM-DD` calendar date, or
    /// `today + intervalDays` leaves years 1 to 9999.
    pub fn check_request(&self, today: &str) -> Result<NewUnitReview, Refusals> {
        let mut refused = Refusals::new();
        if self.interval_days.is_none() && self.scheduled_date.is_none() {
            refused.refuse("intervalDays", RefusalReason::Required, SCHEDULE_REQUIRED);
            return Err(refused);
        }
        if let Some(days) = self.interval_days
            && !(MIN_REVIEW_INTERVAL_DAYS..=MAX_REVIEW_INTERVAL_DAYS).contains(&days)
        {
            refused.refuse(
                "intervalDays",
                RefusalReason::Invalid,
                INTERVAL_OUT_OF_RANGE,
            );
        }
        if let Some(date) = &self.scheduled_date
            && !is_calendar_date(date)
        {
            refused.refuse("scheduledDate", RefusalReason::InvalidFormat, INVALID_DATE);
        }
        refused.into_result()?;

        match (self.interval_days, &self.scheduled_date) {
            (Some(interval_days), Some(date)) => Ok(NewUnitReview {
                scheduled_date: date.clone(),
                interval_days,
            }),
            (Some(interval_days), None) => {
                let Some(scheduled_date) = add_days(today, i64::from(interval_days)) else {
                    let mut refused = Refusals::new();
                    refused.refuse("intervalDays", RefusalReason::Invalid, DATE_OUT_OF_RANGE);
                    return Err(refused);
                };
                Ok(NewUnitReview {
                    scheduled_date,
                    interval_days,
                })
            }
            (None, Some(date)) => {
                let delta =
                    days_between(today, date).unwrap_or(i64::from(MIN_REVIEW_INTERVAL_DAYS));
                let clamped = delta.clamp(
                    i64::from(MIN_REVIEW_INTERVAL_DAYS),
                    i64::from(MAX_REVIEW_INTERVAL_DAYS),
                );
                let interval_days = u32::try_from(clamped).unwrap_or(MIN_REVIEW_INTERVAL_DAYS);
                Ok(NewUnitReview {
                    scheduled_date: date.clone(),
                    interval_days,
                })
            }
            (None, None) => unreachable!("checked above"),
        }
    }
}

/// Request body for completing a personal review.
///
/// Send `{}` (or an empty body) to complete the review without scheduling another, or
/// include `scheduleNextIntervalDays` (`1..=3650`) to schedule the follow-up review in
/// the same transaction.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CompleteUnitReview {
    /// Optional interval in days (`1..=3650`) for scheduling the next review atomically.
    #[serde(default, deserialize_with = "deserialize_non_null_u32")]
    #[schema(value_type = u32, minimum = 1, maximum = 3650)]
    pub schedule_next_interval_days: Option<u32>,
}

impl CompleteUnitReview {
    /// Validates `schedule_next_interval_days` and resolves the follow-up [`NewUnitReview`]
    /// against `today` if requested.
    ///
    /// # Errors
    ///
    /// Returns [`Refusals`] naming `/scheduleNextIntervalDays` if the interval is outside
    /// `1..=3650` or overflows the calendar.
    pub fn check_request(&self, today: &str) -> Result<Option<NewUnitReview>, Refusals> {
        let Some(days) = self.schedule_next_interval_days else {
            return Ok(None);
        };
        let mut refused = Refusals::new();
        if !(MIN_REVIEW_INTERVAL_DAYS..=MAX_REVIEW_INTERVAL_DAYS).contains(&days) {
            refused.refuse(
                "scheduleNextIntervalDays",
                RefusalReason::Invalid,
                INTERVAL_OUT_OF_RANGE,
            );
            return Err(refused);
        }
        let Some(scheduled_date) = add_days(today, i64::from(days)) else {
            refused.refuse(
                "scheduleNextIntervalDays",
                RefusalReason::Invalid,
                DATE_OUT_OF_RANGE,
            );
            return Err(refused);
        };
        Ok(Some(NewUnitReview {
            scheduled_date,
            interval_days: days,
        }))
    }
}

/// Response returned when completing a personal review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CompletedUnitReview {
    /// The completed review record.
    pub review: UnitReview,
    /// Suggested next review interval in days based on the completed review's interval
    /// and the course's interval sequence.
    pub suggested_next_interval_days: u32,
    /// The newly scheduled follow-up review when `scheduleNextIntervalDays` was supplied;
    /// otherwise `null`.
    pub next_review: Option<UnitReview>,
}

/// One pending review due today or overdue for the caller in a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodayReviewItem {
    /// Unique identifier of this review.
    pub id: String,
    /// Identifier of the unit block.
    pub unit_id: String,
    /// Identifier of the course container.
    pub container_id: String,
    /// Current title of the course.
    pub course_title: String,
    /// Current title of the unit.
    pub unit_title: String,
    /// Scheduled calendar date (`YYYY-MM-DD`).
    pub scheduled_date: String,
    /// Review interval in days (`1..=3650`).
    pub interval_days: u32,
    /// Number of days overdue (`0` when due today).
    pub overdue_days: u32,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Last modification time in milliseconds since the Unix epoch.
    pub updated_at: i64,
    /// Suggested next interval in days (`suggested_next_interval_days(Some(interval_days), &review_intervals)`).
    #[serde(default)]
    pub suggested_next_interval_days: u32,
    /// Effective review interval sequence configured on the course.
    #[serde(default)]
    pub review_intervals: Vec<u32>,
}

/// Category of a learner's personal note on a unit.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum UnitNoteKind {
    /// A question to clarify or look up later.
    Question,
    /// A mistake or misconception recorded for review.
    Mistake,
    /// A reflection or summary recorded during review.
    Review,
}

impl UnitNoteKind {
    /// Returns the wire and database form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::Mistake => "mistake",
            Self::Review => "review",
        }
    }
}

impl fmt::Display for UnitNoteKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a unit note kind word is not recognized.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("kind must be one of question, mistake, or review (got {0:?})")]
pub struct ParseUnitNoteKindError(pub String);

impl FromStr for UnitNoteKind {
    type Err = ParseUnitNoteKindError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "question" => Ok(Self::Question),
            "mistake" => Ok(Self::Mistake),
            "review" => Ok(Self::Review),
            other => Err(ParseUnitNoteKindError(other.to_owned())),
        }
    }
}

/// One personal question, mistake, or review note on a course unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnitNote {
    /// Unique identifier of this note.
    pub id: String,
    /// Identifier of the unit block.
    pub unit_id: String,
    /// Identifier of the course container.
    pub container_id: String,
    /// Category of the note (`question`, `mistake`, or `review`).
    pub kind: UnitNoteKind,
    /// Trimmed note content, 1..=10,000 characters.
    pub content: String,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Last modification time in milliseconds since the Unix epoch.
    pub updated_at: i64,
}

/// Request body for creating a personal note on a course unit.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateUnitNote {
    /// Category of the note (`question`, `mistake`, or `review`).
    pub kind: UnitNoteKind,
    /// Note content (trimmed non-empty, at most 10,000 characters).
    pub content: String,
}

/// Validated personal note ready for creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUnitNote {
    /// Category of the note.
    pub kind: UnitNoteKind,
    /// Trimmed and validated content.
    pub content: String,
}

impl CreateUnitNote {
    /// Validates and normalizes the note creation payload.
    ///
    /// # Errors
    ///
    /// Returns [`Refusals`] naming `/content` if it is blank after trimming, exceeds
    /// [`UNIT_NOTE_CONTENT_MAX_CHARS`] characters, or contains disallowed control
    /// characters.
    pub fn check_request(&self) -> Result<NewUnitNote, Refusals> {
        let content = validate_note_content(&self.content)?;
        Ok(NewUnitNote {
            kind: self.kind,
            content,
        })
    }
}

/// Request body for updating a personal note on a course unit.
///
/// Omitted fields leave the stored value unchanged; `null` is refused.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchUnitNote {
    /// Replacement note category; omit to keep unchanged.
    #[serde(default, deserialize_with = "deserialize_non_null_note_kind")]
    #[schema(value_type = UnitNoteKind)]
    pub kind: Option<UnitNoteKind>,
    /// Replacement note content (trimmed non-empty, at most 10,000 characters); omit to
    /// keep unchanged.
    #[serde(default, deserialize_with = "deserialize_non_null_string")]
    #[schema(value_type = String)]
    pub content: Option<String>,
}

/// Validated personal note patch ready for storage.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnitNoteChange {
    /// Replacement kind if provided.
    pub kind: Option<UnitNoteKind>,
    /// Replacement trimmed content if provided.
    pub content: Option<String>,
}

impl PatchUnitNote {
    /// Validates and normalizes the note patch payload.
    ///
    /// # Errors
    ///
    /// Returns [`Refusals`] naming `/content` if `content` is present and invalid.
    pub fn check_request(&self) -> Result<UnitNoteChange, Refusals> {
        let content = self
            .content
            .as_deref()
            .map(validate_note_content)
            .transpose()?;
        Ok(UnitNoteChange {
            kind: self.kind,
            content,
        })
    }
}

fn validate_note_content(raw: &str) -> Result<String, Refusals> {
    let mut refused = Refusals::new();
    if raw
        .chars()
        .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
    {
        refused.refuse(
            "content",
            RefusalReason::InvalidCharacters,
            CONTENT_CONTROL_CHARS,
        );
        return Err(refused);
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        refused.refuse("content", RefusalReason::Required, CONTENT_EMPTY);
    } else if trimmed.chars().count() > UNIT_NOTE_CONTENT_MAX_CHARS {
        refused.refuse("content", RefusalReason::TooLong, CONTENT_TOO_LONG);
    }
    refused.into_result()?;
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

    use super::*;

    #[test]
    fn suggested_interval_follows_default_and_custom_sequences() {
        let default = &DEFAULT_REVIEW_INTERVALS;
        assert_eq!(suggested_next_interval_days(None, default), 1);
        assert_eq!(suggested_next_interval_days(Some(1), default), 3);
        assert_eq!(suggested_next_interval_days(Some(3), default), 7);
        assert_eq!(suggested_next_interval_days(Some(7), default), 14);
        assert_eq!(suggested_next_interval_days(Some(14), default), 30);
        assert_eq!(suggested_next_interval_days(Some(30), default), 30);
        assert_eq!(suggested_next_interval_days(Some(60), default), 30);
        assert_eq!(suggested_next_interval_days(Some(2), default), 3);
        assert_eq!(suggested_next_interval_days(Some(10), default), 14);

        let custom = [12, 2, 5];
        assert_eq!(suggested_next_interval_days(None, &custom), 2);
        assert_eq!(suggested_next_interval_days(Some(2), &custom), 5);
        assert_eq!(suggested_next_interval_days(Some(5), &custom), 12);
        assert_eq!(suggested_next_interval_days(Some(12), &custom), 12);
        assert_eq!(ReviewStatusFilter::Pending.to_string(), "pending");
    }

    #[test]
    fn course_review_intervals_reads_valid_property_or_falls_back() {
        assert_eq!(
            course_review_intervals(&Map::new()),
            DEFAULT_REVIEW_INTERVALS.to_vec()
        );
        let props: JsonMap = serde_json::from_value(json!({
            "defaultReviewIntervals": [10, 2, 10, 0, 5000, "bad", 4]
        }))
        .expect("json map");
        assert_eq!(course_review_intervals(&props), vec![2, 4, 10]);

        let invalid: JsonMap =
            serde_json::from_value(json!({"defaultReviewIntervals": [0, -1, 4000]}))
                .expect("json map");
        assert_eq!(
            course_review_intervals(&invalid),
            DEFAULT_REVIEW_INTERVALS.to_vec()
        );
    }

    #[test]
    fn schedule_request_resolves_interval_and_date_and_rejects_invalid() {
        let by_interval: ScheduleUnitReview =
            serde_json::from_value(json!({"intervalDays": 7})).expect("valid");
        assert_eq!(
            by_interval.check_request("2026-11-24"),
            Ok(NewUnitReview {
                scheduled_date: "2026-12-01".to_owned(),
                interval_days: 7,
            })
        );

        let by_date: ScheduleUnitReview =
            serde_json::from_value(json!({"scheduledDate": "2026-11-27"})).expect("valid");
        assert_eq!(
            by_date.check_request("2026-11-24"),
            Ok(NewUnitReview {
                scheduled_date: "2026-11-27".to_owned(),
                interval_days: 3,
            })
        );

        let past_date: ScheduleUnitReview =
            serde_json::from_value(json!({"scheduledDate": "2026-11-20"})).expect("valid");
        assert_eq!(
            past_date.check_request("2026-11-24"),
            Ok(NewUnitReview {
                scheduled_date: "2026-11-20".to_owned(),
                interval_days: 1,
            })
        );

        let both: ScheduleUnitReview =
            serde_json::from_value(json!({"intervalDays": 14, "scheduledDate": "2026-12-10"}))
                .expect("valid");
        assert_eq!(
            both.check_request("2026-11-24"),
            Ok(NewUnitReview {
                scheduled_date: "2026-12-10".to_owned(),
                interval_days: 14,
            })
        );

        let empty: ScheduleUnitReview = serde_json::from_value(json!({})).expect("empty object");
        assert!(empty.check_request("2026-11-24").is_err());

        for bad in [
            json!({"intervalDays": 0}),
            json!({"intervalDays": 3651}),
            json!({"scheduledDate": "2026-02-30"}),
            json!({"scheduledDate": "not-a-date"}),
        ] {
            let parsed: ScheduleUnitReview = serde_json::from_value(bad.clone()).expect("parsed");
            assert!(parsed.check_request("2026-11-24").is_err(), "{bad}");
        }

        assert!(
            serde_json::from_value::<ScheduleUnitReview>(json!({"intervalDays": null})).is_err()
        );
        assert!(serde_json::from_value::<ScheduleUnitReview>(json!({"extra": 1})).is_err());
    }

    #[test]
    fn complete_request_validates_optional_next_interval() {
        let empty: CompleteUnitReview = serde_json::from_value(json!({})).expect("empty");
        assert_eq!(empty.check_request("2026-11-24"), Ok(None));

        let next: CompleteUnitReview =
            serde_json::from_value(json!({"scheduleNextIntervalDays": 14})).expect("valid");
        assert_eq!(
            next.check_request("2026-11-24"),
            Ok(Some(NewUnitReview {
                scheduled_date: "2026-12-08".to_owned(),
                interval_days: 14,
            }))
        );

        for bad in [
            json!({"scheduleNextIntervalDays": 0}),
            json!({"scheduleNextIntervalDays": 3651}),
        ] {
            let parsed: CompleteUnitReview = serde_json::from_value(bad.clone()).expect("parsed");
            assert!(parsed.check_request("2026-11-24").is_err(), "{bad}");
        }
        assert!(
            serde_json::from_value::<CompleteUnitReview>(json!({"scheduleNextIntervalDays": null}))
                .is_err()
        );
    }

    #[test]
    fn note_validation_trims_and_rejects_blank_long_and_control_chars() {
        let created: CreateUnitNote = serde_json::from_value(json!({
            "kind": "mistake",
            "content": "  line 1\n\tline 2  "
        }))
        .expect("valid");
        assert_eq!(
            created.check_request(),
            Ok(NewUnitNote {
                kind: UnitNoteKind::Mistake,
                content: "line 1\n\tline 2".to_owned(),
            })
        );

        let blank: CreateUnitNote =
            serde_json::from_value(json!({"kind": "question", "content": "   \n\t  "}))
                .expect("parsed");
        assert!(blank.check_request().is_err());

        let exact = "가".repeat(UNIT_NOTE_CONTENT_MAX_CHARS);
        let at_limit: CreateUnitNote =
            serde_json::from_value(json!({"kind": "review", "content": format!(" {exact} ")}))
                .expect("parsed");
        assert_eq!(at_limit.check_request().expect("at limit").content, exact);

        let too_long: CreateUnitNote = serde_json::from_value(json!({
            "kind": "review",
            "content": "가".repeat(UNIT_NOTE_CONTENT_MAX_CHARS + 1)
        }))
        .expect("parsed");
        assert!(too_long.check_request().is_err());

        let nul: CreateUnitNote =
            serde_json::from_value(json!({"kind": "question", "content": "a\0b"})).expect("parsed");
        assert!(nul.check_request().is_err());

        assert!(serde_json::from_value::<PatchUnitNote>(json!({"kind": null})).is_err());
        assert!(serde_json::from_value::<PatchUnitNote>(json!({"content": null})).is_err());
    }
}
