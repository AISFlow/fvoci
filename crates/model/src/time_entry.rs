//! Personal time tracking entries and single active timer wire types.
//!
//! Every entry belongs solely to its user. A user may have at most one active (running)
//! entry (`ended_at: None`) at a time across all organizations; starting a new timer
//! stops the previous one atomically.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::refusal::{RefusalReason, Refusals};

/// Maximum number of characters allowed in a time entry's note.
pub const TIME_ENTRY_NOTE_MAX_CHARS: usize = 1000;

/// Largest accepted epoch millisecond timestamp (the ECMAScript `Date` range limit).
///
/// Bounding timestamps to `0..=MAX_TIME_MILLIS` imposes no practical duration cap while
/// keeping timestamps representable as exact JavaScript numbers and preventing signed
/// 64-bit subtraction overflow in duration calculations.
pub const MAX_TIME_MILLIS: i64 = 8_640_000_000_000_000;

const NOTE_TOO_LONG: &str = "at most 1000 characters";
const NOTE_WITH_NUL: &str = "the text holds the character U+0000";
const TIME_OUT_OF_RANGE: &str = "time must be between 0 and 8640000000000000 milliseconds";
const ENDED_BEFORE_STARTED: &str = "endedAt must be greater than or equal to startedAt";

/// What kind of activity a time entry records.
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
pub enum TimeEntryKind {
    /// Attending or watching a lecture.
    Lecture,
    /// Self-directed study or review.
    Study,
    /// General task or project work.
    Work,
    /// Any other tracked activity.
    Other,
}

impl TimeEntryKind {
    /// Every supported activity kind, in display order.
    pub const ALL: [Self; 4] = [Self::Lecture, Self::Study, Self::Work, Self::Other];

    /// The wire and database string for this kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Lecture => "lecture",
            Self::Study => "study",
            Self::Work => "work",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for TimeEntryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a string names no [`TimeEntryKind`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown time entry kind {0:?}")]
pub struct ParseTimeEntryKindError(pub String);

impl FromStr for TimeEntryKind {
    type Err = ParseTimeEntryKindError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or_else(|| ParseTimeEntryKindError(text.to_owned()))
    }
}

/// What kind of resource a time entry was linked to when created.
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
pub enum TimeTargetKind {
    /// A non-template card on a board.
    Card,
    /// A unit in a course.
    Unit,
    /// A board container (without a specific card).
    Board,
    /// A course container (without a specific unit).
    Course,
}

impl TimeTargetKind {
    /// Every supported target kind.
    pub const ALL: [Self; 4] = [Self::Card, Self::Unit, Self::Board, Self::Course];

    /// The wire and database string for this target kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Card => "card",
            Self::Unit => "unit",
            Self::Board => "board",
            Self::Course => "course",
        }
    }
}

impl fmt::Display for TimeTargetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a string names no [`TimeTargetKind`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown time target kind {0:?}")]
pub struct ParseTimeTargetKindError(pub String);

impl FromStr for TimeTargetKind {
    type Err = ParseTimeTargetKindError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or_else(|| ParseTimeTargetKindError(text.to_owned()))
    }
}

/// One personal time tracking entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimeEntry {
    /// The entry's identifier.
    pub id: String,
    /// The workspace of the linked target, or `null` for a targetless entry.
    pub workspace_id: Option<String>,
    /// The board or course of the linked target, or `null` if none.
    pub container_id: Option<String>,
    /// The card or unit linked to this entry, or `null` if none. Preserved even if
    /// the target block is later moved to the trash or permanently deleted.
    pub block_id: Option<String>,
    /// What kind of resource the entry was linked to (`card`, `unit`, `board`, `course`),
    /// or `null` for a targetless or workspace-only entry.
    pub target_kind: Option<TimeTargetKind>,
    /// Snapshot of the target's title taken when the entry was recorded, or `null` if
    /// targetless. Preserved when the target is later deleted.
    pub target_title: Option<String>,
    /// Activity category (`lecture`, `study`, `work`, `other`).
    pub kind: TimeEntryKind,
    /// When the entry started, in milliseconds since the Unix epoch.
    pub started_at: i64,
    /// When the entry ended, in milliseconds since the Unix epoch; `null` while running.
    pub ended_at: Option<i64>,
    /// Optional note written by the user (at most 1000 characters).
    pub note: String,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Last modification time in milliseconds since the Unix epoch.
    pub updated_at: i64,
}

/// The caller's currently running timer, if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActiveTimeEntry {
    /// The running entry visible to this session, or `null` if no timer is running.
    pub entry: Option<TimeEntry>,
}

/// Result of starting a timer: the newly started entry and, if a previous active timer
/// in an organization open to this session was stopped automatically, that stopped entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StartedTimeEntry {
    /// The newly started active timer (`endedAt` is `null`).
    pub entry: TimeEntry,
    /// The previous active timer that was stopped in the same transaction, if one was
    /// running in an organization open to this session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub stopped: Option<TimeEntry>,
}

/// Total completed duration in milliseconds broken down by [`TimeEntryKind`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimeEntryKindTotals {
    /// Completed milliseconds logged as `lecture`.
    pub lecture: i64,
    /// Completed milliseconds logged as `study`.
    pub study: i64,
    /// Completed milliseconds logged as `work`.
    pub work: i64,
    /// Completed milliseconds logged as `other`.
    pub other: i64,
}

impl TimeEntryKindTotals {
    /// Adds `duration_ms` to the bucket for `kind`, saturating at `i64::MAX`.
    pub fn add(&mut self, kind: TimeEntryKind, duration_ms: i64) {
        let slot = match kind {
            TimeEntryKind::Lecture => &mut self.lecture,
            TimeEntryKind::Study => &mut self.study,
            TimeEntryKind::Work => &mut self.work,
            TimeEntryKind::Other => &mut self.other,
        };
        *slot = slot.saturating_add(duration_ms.max(0));
    }

    /// Sum of all four activity buckets, saturating at `i64::MAX`.
    #[must_use]
    pub const fn total(self) -> i64 {
        self.lecture
            .saturating_add(self.study)
            .saturating_add(self.work)
            .saturating_add(self.other)
    }
}

/// Aggregated summary across all matching entries (across all pages).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimeEntrySummary {
    /// Total completed duration in milliseconds (running timers contribute 0 until stopped).
    pub total_ms: i64,
    /// Completed duration in milliseconds per activity kind.
    pub by_kind: TimeEntryKindTotals,
    /// Total number of matching entries across all pages (including any running entry).
    pub count: i64,
}

/// One page of personal time entries paired with the aggregate summary of the full filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TimeEntryList {
    /// The requested page of entries, ordered newest first (`startedAt DESC, id DESC`).
    pub entries: Vec<TimeEntry>,
    /// Totals across every entry matching the query filters.
    pub summary: TimeEntrySummary,
}

/// Request body for starting a live timer (`POST /users/me/time/start`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartTimeEntryRequest {
    /// Optional workspace identifier of the target.
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// Optional board or course identifier of the target.
    #[serde(default)]
    pub container_id: Option<String>,
    /// Optional card (non-template) or unit identifier to track time against.
    #[serde(default)]
    pub block_id: Option<String>,
    /// Activity category (`lecture`, `study`, `work`, `other`).
    pub kind: TimeEntryKind,
    /// Optional note (at most 1000 characters).
    #[serde(default)]
    pub note: String,
}

impl StartTimeEntryRequest {
    /// Validates request fields before target lookup.
    ///
    /// # Errors
    ///
    /// Returns [`Refusals`] if `note` exceeds 1000 characters or contains U+0000.
    pub fn check_request(&self) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        refuse_note(&mut refused, Some(&self.note));
        refused.into_result()
    }
}

/// Request body for manually logging a completed time entry (`POST /users/me/time/entries`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateTimeEntryRequest {
    /// Optional workspace identifier of the target.
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// Optional board or course identifier of the target.
    #[serde(default)]
    pub container_id: Option<String>,
    /// Optional card (non-template) or unit identifier to link.
    #[serde(default)]
    pub block_id: Option<String>,
    /// Activity category (`lecture`, `study`, `work`, `other`).
    pub kind: TimeEntryKind,
    /// Start timestamp in milliseconds since the Unix epoch.
    pub started_at: i64,
    /// End timestamp in milliseconds since the Unix epoch (`>= startedAt`).
    pub ended_at: i64,
    /// Optional note (at most 1000 characters).
    #[serde(default)]
    pub note: String,
}

impl CreateTimeEntryRequest {
    /// Validates request fields before target lookup.
    ///
    /// # Errors
    ///
    /// Returns [`Refusals`] if timestamps are out of range, `endedAt < startedAt`, or
    /// `note` is invalid.
    pub fn check_request(&self) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        refuse_timestamp(&mut refused, "startedAt", self.started_at);
        refuse_timestamp(&mut refused, "endedAt", self.ended_at);
        let valid = 0..=MAX_TIME_MILLIS;
        if valid.contains(&self.started_at)
            && valid.contains(&self.ended_at)
            && self.ended_at < self.started_at
        {
            refused.refuse("endedAt", RefusalReason::Invalid, ENDED_BEFORE_STARTED);
        }
        refuse_note(&mut refused, Some(&self.note));
        refused.into_result()
    }
}

/// Deserializes a present field as `Some(T)`, refusing explicit JSON `null`.
fn non_null<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// Request body for editing one of the user's time entries (`PATCH /users/me/time/entries/{entryId}`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct TimeEntryPatch {
    /// New activity category.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "non_null")]
    #[schema(nullable = false)]
    pub kind: Option<TimeEntryKind>,
    /// New start timestamp in milliseconds since the Unix epoch.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "non_null")]
    #[schema(nullable = false)]
    pub started_at: Option<i64>,
    /// New end timestamp in milliseconds since the Unix epoch. Setting this on a running
    /// timer stops it at the given timestamp.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "non_null")]
    #[schema(nullable = false)]
    pub ended_at: Option<i64>,
    /// New note (at most 1000 characters).
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "non_null")]
    #[schema(nullable = false)]
    pub note: Option<String>,
}

impl TimeEntryPatch {
    /// Validates patch fields independent of the existing stored entry.
    ///
    /// # Errors
    ///
    /// Returns [`Refusals`] if any timestamp is out of range, both timestamps are given
    /// with `endedAt < startedAt`, or `note` is invalid.
    pub fn check_request(&self) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        if let Some(started_at) = self.started_at {
            refuse_timestamp(&mut refused, "startedAt", started_at);
        }
        if let Some(ended_at) = self.ended_at {
            refuse_timestamp(&mut refused, "endedAt", ended_at);
        }
        let valid = 0..=MAX_TIME_MILLIS;
        if let (Some(started_at), Some(ended_at)) = (self.started_at, self.ended_at)
            && valid.contains(&started_at)
            && valid.contains(&ended_at)
            && ended_at < started_at
        {
            refused.refuse("endedAt", RefusalReason::Invalid, ENDED_BEFORE_STARTED);
        }
        refuse_note(&mut refused, self.note.as_deref());
        refused.into_result()
    }
}

fn refuse_timestamp(refused: &mut Refusals, member: &str, value: i64) {
    if !(0..=MAX_TIME_MILLIS).contains(&value) {
        refused.refuse(member, RefusalReason::Invalid, TIME_OUT_OF_RANGE);
    }
}

fn refuse_note(refused: &mut Refusals, text: Option<&str>) {
    let Some(text) = text else {
        return;
    };
    if text.chars().count() > TIME_ENTRY_NOTE_MAX_CHARS {
        refused.refuse("note", RefusalReason::TooLong, NOTE_TOO_LONG);
    } else if text.contains('\0') {
        refused.refuse("note", RefusalReason::InvalidCharacters, NOTE_WITH_NUL);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn every_kind_round_trips_through_its_wire_string() {
        for kind in TimeEntryKind::ALL {
            assert_eq!(kind.as_str().parse(), Ok(kind));
            assert_eq!(kind.to_string(), kind.as_str());
            assert_eq!(
                serde_json::to_value(kind).expect("serialize kind"),
                json!(kind.as_str())
            );
        }
        assert!("meeting".parse::<TimeEntryKind>().is_err());

        for target_kind in TimeTargetKind::ALL {
            assert_eq!(target_kind.as_str().parse(), Ok(target_kind));
            assert_eq!(target_kind.to_string(), target_kind.as_str());
            assert_eq!(
                serde_json::to_value(target_kind).expect("serialize target kind"),
                json!(target_kind.as_str())
            );
        }
        assert!("chapter".parse::<TimeTargetKind>().is_err());
    }

    #[test]
    fn create_request_validates_timestamps_and_note() {
        let valid = CreateTimeEntryRequest {
            workspace_id: None,
            container_id: None,
            block_id: None,
            kind: TimeEntryKind::Study,
            started_at: 1000,
            ended_at: 1000,
            note: "가".repeat(TIME_ENTRY_NOTE_MAX_CHARS),
        };
        assert_eq!(valid.check_request(), Ok(()));

        let bad = CreateTimeEntryRequest {
            started_at: 2000,
            ended_at: 1000,
            note: "a".repeat(TIME_ENTRY_NOTE_MAX_CHARS + 1),
            ..valid
        };
        let refused = bad.check_request().expect_err("invalid create request");
        let fields: Vec<_> = refused
            .iter()
            .map(|r| (r.pointer.as_str(), r.reason))
            .collect();
        assert_eq!(
            fields,
            [
                ("/endedAt", RefusalReason::Invalid),
                ("/note", RefusalReason::TooLong),
            ]
        );

        let out_of_range = CreateTimeEntryRequest {
            started_at: 1_000,
            ended_at: -1,
            note: String::new(),
            ..bad
        };
        let single = out_of_range
            .check_request()
            .expect_err("negative endedAt refused");
        assert_eq!(
            single.iter().count(),
            1,
            "out-of-range endedAt must not emit a duplicate /endedAt refusal"
        );
    }

    #[test]
    fn patch_rejects_explicit_null_and_unknown_fields() {
        let empty: TimeEntryPatch = serde_json::from_value(json!({})).expect("empty patch");
        assert_eq!(empty, TimeEntryPatch::default());
        assert!(serde_json::from_value::<TimeEntryPatch>(json!({"endedAt": null})).is_err());
        assert!(serde_json::from_value::<TimeEntryPatch>(json!({"kind": null})).is_err());
        assert!(serde_json::from_value::<TimeEntryPatch>(json!({"extra": 1})).is_err());

        let with_nul: TimeEntryPatch =
            serde_json::from_value(json!({"note": "bad\0note"})).expect("deserialized");
        let refused = with_nul.check_request().expect_err("nul rejected");
        assert_eq!(
            refused.iter().next().map(|r| r.reason),
            Some(RefusalReason::InvalidCharacters)
        );
    }

    #[test]
    fn kind_totals_saturate_on_overflow() {
        let mut totals = TimeEntryKindTotals::default();
        totals.add(TimeEntryKind::Lecture, i64::MAX - 5);
        totals.add(TimeEntryKind::Study, 100);
        totals.add(TimeEntryKind::Work, -50);
        assert_eq!(totals.work, 0);
        assert_eq!(totals.total(), i64::MAX);
    }
}
