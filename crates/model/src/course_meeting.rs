//! Course study meetings and attendance records.
//!
//! Meetings are shared among members of a course (`ContainerKind::Course`). Attendance
//! rows are shared across the course and kept as tombstones when an account is erased.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

use crate::date_value::LAST_MILLISECOND;
use crate::id::is_id;
use crate::refusal::{RefusalReason, Refusals};

/// Maximum length in Unicode scalar values of [`CourseMeeting::title`].
pub const COURSE_MEETING_TITLE_MAX_CHARS: usize = 200;

/// Maximum length in Unicode scalar values of [`CourseMeeting::location`].
pub const COURSE_MEETING_LOCATION_MAX_CHARS: usize = 500;

/// Maximum length in Unicode scalar values of [`CourseMeeting::notes`].
pub const COURSE_MEETING_NOTES_MAX_CHARS: usize = 5000;

/// Maximum number of unit identifiers linked to a single meeting.
pub const COURSE_MEETING_MAX_UNITS: usize = 200;

const TITLE_REQUIRED: &str = "meeting title is required";
const TITLE_TOO_LONG: &str = "at most 200 characters";
const LOCATION_TOO_LONG: &str = "at most 500 characters";
const NOTES_TOO_LONG: &str = "at most 5000 characters";
const CONTROL_CHARS: &str = "control characters are not allowed";
const STARTS_AT_RANGE: &str = "startsAt must be milliseconds from 0 to the end of 9999";
const UNIT_IDS_TOO_MANY: &str = "at most 200 unit identifiers";
const INVALID_ID: &str = "must be a valid identifier";

/// A member's attendance status for a course study meeting.
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
pub enum AttendanceStatus {
    /// Attended on time.
    Present,
    /// Attended late.
    Late,
    /// Did not attend.
    Absent,
    /// Excused absence.
    Excused,
}

impl AttendanceStatus {
    /// The wire and database spelling of the status.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Late => "late",
            Self::Absent => "absent",
            Self::Excused => "excused",
        }
    }
}

impl fmt::Display for AttendanceStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a string is not a known [`AttendanceStatus`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown attendance status {0:?}")]
pub struct UnknownAttendanceStatus(pub String);

impl FromStr for AttendanceStatus {
    type Err = UnknownAttendanceStatus;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "present" => Ok(Self::Present),
            "late" => Ok(Self::Late),
            "absent" => Ok(Self::Absent),
            "excused" => Ok(Self::Excused),
            other => Err(UnknownAttendanceStatus(other.to_owned())),
        }
    }
}

/// One member's attendance record on a study meeting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MeetingAttendance {
    /// Identifier of the course member (retained as a tombstone if the account is erased).
    pub user_id: String,
    /// Recorded attendance status.
    pub status: AttendanceStatus,
    /// Last modification time in milliseconds since the Unix epoch (`0` when hidden from
    /// non-administrators).
    pub updated_at: i64,
}

/// A study meeting belonging to a course, together with its attendance records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CourseMeeting {
    /// Identifier of the meeting.
    pub id: String,
    /// Identifier of the course container.
    pub container_id: String,
    /// Meeting title, at most 200 characters.
    pub title: String,
    /// Scheduled start time in milliseconds since the Unix epoch.
    pub starts_at: i64,
    /// Meeting location or URL, at most 500 characters; empty if unset.
    pub location: String,
    /// Identifiers of units covered by this meeting.
    pub unit_ids: Vec<String>,
    /// Optional identifier of the presenter.
    pub presenter_id: Option<String>,
    /// Optional identifier of the host.
    pub host_id: Option<String>,
    /// Shared meeting notes, at most 5000 characters; empty if unset.
    pub notes: String,
    /// When the upcoming-meeting notification was sent, or `null` if not yet sent.
    pub notified_at: Option<i64>,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Last modification time in milliseconds since the Unix epoch.
    pub updated_at: i64,
    /// Attendance records for this meeting, ordered by `userId`.
    pub attendance: Vec<MeetingAttendance>,
}

/// How a patch changes an optional identifier field (`presenterId` or `hostId`).
///
/// Omitting the field keeps the existing value, `null` clears it, and a string replaces it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum OptionalIdChange {
    /// Leave the stored identifier unchanged.
    #[default]
    Keep,
    /// Clear the stored identifier to `None`.
    Clear,
    /// Replace the stored identifier.
    Set(String),
}

impl Serialize for OptionalIdChange {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Keep | Self::Clear => serializer.serialize_none(),
            Self::Set(id) => id.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for OptionalIdChange {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Option::<String>::deserialize(deserializer)?.map_or(Self::Clear, Self::Set))
    }
}

fn deserialize_non_null_string<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

fn deserialize_non_null_i64<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<i64>, D::Error> {
    i64::deserialize(deserializer).map(Some)
}

fn deserialize_non_null_vec_string<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error> {
    Vec::<String>::deserialize(deserializer).map(Some)
}

/// Request body for creating a course study meeting.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewCourseMeeting {
    /// Meeting title (required, 1–200 characters, no control characters).
    pub title: String,
    /// Scheduled start time in milliseconds since the Unix epoch (`0..=253402300799999`).
    pub starts_at: i64,
    /// Meeting location or URL (at most 500 characters, no control characters).
    #[serde(default)]
    pub location: String,
    /// Identifiers of units covered by this meeting (at most 200 valid identifiers).
    #[serde(default)]
    pub unit_ids: Vec<String>,
    /// Optional presenter user identifier.
    #[serde(default)]
    pub presenter_id: Option<String>,
    /// Optional host user identifier.
    #[serde(default)]
    pub host_id: Option<String>,
    /// Shared meeting notes (at most 5000 characters).
    #[serde(default)]
    pub notes: String,
}

/// Validated new course meeting ready for storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedNewMeeting {
    /// Trimmed, validated title.
    pub title: String,
    /// Validated start time in milliseconds since the Unix epoch.
    pub starts_at: i64,
    /// Trimmed, validated location.
    pub location: String,
    /// Deduplicated, validated unit identifiers.
    pub unit_ids: Vec<String>,
    /// Optional validated presenter identifier.
    pub presenter_id: Option<String>,
    /// Optional validated host identifier.
    pub host_id: Option<String>,
    /// Validated notes.
    pub notes: String,
}

impl NewCourseMeeting {
    /// Validates and normalizes the request body into a [`ValidatedNewMeeting`].
    ///
    /// # Errors
    ///
    /// Returns [`Refusals`] naming every invalid field.
    pub fn check_request(&self) -> Result<ValidatedNewMeeting, Refusals> {
        let mut refused = Refusals::new();
        let title = check_title(&self.title, &mut refused);
        check_starts_at(self.starts_at, &mut refused);
        let location = check_location(&self.location, &mut refused);
        let unit_ids = check_unit_ids(&self.unit_ids, &mut refused);
        let presenter_id =
            check_optional_id("presenterId", self.presenter_id.as_deref(), &mut refused);
        let host_id = check_optional_id("hostId", self.host_id.as_deref(), &mut refused);
        let notes = check_notes(&self.notes, &mut refused);
        refused.into_result()?;
        Ok(ValidatedNewMeeting {
            title,
            starts_at: self.starts_at,
            location,
            unit_ids,
            presenter_id,
            host_id,
            notes,
        })
    }
}

/// Request body for patching a course study meeting.
///
/// Omitted fields are left unchanged. Sending `null` for `presenterId` or `hostId` clears
/// that assignment; sending `null` for non-nullable fields is refused.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CourseMeetingPatch {
    /// Replacement title (1–200 characters, no control characters).
    #[serde(default, deserialize_with = "deserialize_non_null_string")]
    #[schema(value_type = String)]
    pub title: Option<String>,
    /// Replacement start time in milliseconds since the Unix epoch.
    #[serde(default, deserialize_with = "deserialize_non_null_i64")]
    #[schema(value_type = i64)]
    pub starts_at: Option<i64>,
    /// Replacement location or URL (at most 500 characters, no control characters).
    #[serde(default, deserialize_with = "deserialize_non_null_string")]
    #[schema(value_type = String)]
    pub location: Option<String>,
    /// Replacement unit identifier list (at most 200 valid identifiers).
    #[serde(default, deserialize_with = "deserialize_non_null_vec_string")]
    #[schema(value_type = Vec<String>)]
    pub unit_ids: Option<Vec<String>>,
    /// Replacement presenter identifier; `null` clears it.
    #[schema(value_type = Option<String>)]
    pub presenter_id: OptionalIdChange,
    /// Replacement host identifier; `null` clears it.
    #[schema(value_type = Option<String>)]
    pub host_id: OptionalIdChange,
    /// Replacement notes (at most 5000 characters).
    #[serde(default, deserialize_with = "deserialize_non_null_string")]
    #[schema(value_type = String)]
    pub notes: Option<String>,
}

/// Validated meeting patch ready for storage.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CourseMeetingChange {
    /// Replacement title, if changing.
    pub title: Option<String>,
    /// Replacement start time, if changing.
    pub starts_at: Option<i64>,
    /// Replacement location, if changing.
    pub location: Option<String>,
    /// Replacement unit identifiers, if changing.
    pub unit_ids: Option<Vec<String>>,
    /// Presenter identifier change.
    pub presenter_id: OptionalIdChange,
    /// Host identifier change.
    pub host_id: OptionalIdChange,
    /// Replacement notes, if changing.
    pub notes: Option<String>,
}

impl CourseMeetingPatch {
    /// Validates and normalizes the patch into a [`CourseMeetingChange`].
    ///
    /// # Errors
    ///
    /// Returns [`Refusals`] naming every invalid field.
    pub fn check_request(&self) -> Result<CourseMeetingChange, Refusals> {
        let mut refused = Refusals::new();
        let title = self
            .title
            .as_deref()
            .map(|raw| check_title(raw, &mut refused));
        if let Some(starts_at) = self.starts_at {
            check_starts_at(starts_at, &mut refused);
        }
        let location = self
            .location
            .as_deref()
            .map(|raw| check_location(raw, &mut refused));
        let unit_ids = self
            .unit_ids
            .as_deref()
            .map(|ids| check_unit_ids(ids, &mut refused));
        let presenter_id = check_id_change("presenterId", &self.presenter_id, &mut refused);
        let host_id = check_id_change("hostId", &self.host_id, &mut refused);
        let notes = self
            .notes
            .as_deref()
            .map(|raw| check_notes(raw, &mut refused));
        refused.into_result()?;
        Ok(CourseMeetingChange {
            title,
            starts_at: self.starts_at,
            location,
            unit_ids,
            presenter_id,
            host_id,
            notes,
        })
    }
}

/// Request body for recording a member's attendance on a course meeting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeetingAttendanceUpdate {
    /// Attendance status (`present`, `late`, `absent`, or `excused`).
    pub status: AttendanceStatus,
}

fn check_title(raw: &str, refused: &mut Refusals) -> String {
    if raw.chars().any(char::is_control) {
        refused.refuse("title", RefusalReason::InvalidCharacters, CONTROL_CHARS);
        return String::new();
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        refused.refuse("title", RefusalReason::Required, TITLE_REQUIRED);
    } else if trimmed.chars().count() > COURSE_MEETING_TITLE_MAX_CHARS {
        refused.refuse("title", RefusalReason::TooLong, TITLE_TOO_LONG);
    }
    trimmed.to_owned()
}

fn check_starts_at(starts_at: i64, refused: &mut Refusals) {
    if !(0..=LAST_MILLISECOND).contains(&starts_at) {
        refused.refuse("startsAt", RefusalReason::Invalid, STARTS_AT_RANGE);
    }
}

fn check_location(raw: &str, refused: &mut Refusals) -> String {
    if raw.chars().any(char::is_control) {
        refused.refuse("location", RefusalReason::InvalidCharacters, CONTROL_CHARS);
        return String::new();
    }
    let trimmed = raw.trim();
    if trimmed.chars().count() > COURSE_MEETING_LOCATION_MAX_CHARS {
        refused.refuse("location", RefusalReason::TooLong, LOCATION_TOO_LONG);
    }
    trimmed.to_owned()
}

fn check_unit_ids(ids: &[String], refused: &mut Refusals) -> Vec<String> {
    if ids.len() > COURSE_MEETING_MAX_UNITS {
        refused.refuse("unitIds", RefusalReason::TooLong, UNIT_IDS_TOO_MANY);
        return Vec::new();
    }
    let mut seen = HashSet::with_capacity(ids.len());
    let mut out = Vec::with_capacity(ids.len());
    for (index, id) in ids.iter().enumerate() {
        if !is_id(id) {
            refused.refuse_at(
                format!("/unitIds/{index}"),
                RefusalReason::InvalidFormat,
                INVALID_ID,
            );
        } else if seen.insert(id.as_str()) {
            out.push(id.clone());
        }
    }
    out
}

fn check_optional_id(field: &str, value: Option<&str>, refused: &mut Refusals) -> Option<String> {
    let raw = value?;
    if !is_id(raw) {
        refused.refuse(field, RefusalReason::InvalidFormat, INVALID_ID);
        return None;
    }
    Some(raw.to_owned())
}

fn check_id_change(
    field: &str,
    change: &OptionalIdChange,
    refused: &mut Refusals,
) -> OptionalIdChange {
    match change {
        OptionalIdChange::Keep => OptionalIdChange::Keep,
        OptionalIdChange::Clear => OptionalIdChange::Clear,
        OptionalIdChange::Set(raw) => {
            if is_id(raw) {
                OptionalIdChange::Set(raw.clone())
            } else {
                refused.refuse(field, RefusalReason::InvalidFormat, INVALID_ID);
                OptionalIdChange::Keep
            }
        }
    }
}

fn check_notes(raw: &str, refused: &mut Refusals) -> String {
    if raw
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        refused.refuse("notes", RefusalReason::InvalidCharacters, CONTROL_CHARS);
        return String::new();
    }
    if raw.chars().count() > COURSE_MEETING_NOTES_MAX_CHARS {
        refused.refuse("notes", RefusalReason::TooLong, NOTES_TOO_LONG);
    }
    raw.to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::new_id;

    #[test]
    fn attendance_status_round_trips() {
        for status in [
            AttendanceStatus::Present,
            AttendanceStatus::Late,
            AttendanceStatus::Absent,
            AttendanceStatus::Excused,
        ] {
            assert_eq!(status.as_str().parse(), Ok(status));
            assert_eq!(
                serde_json::to_value(status).expect("json"),
                json!(status.as_str())
            );
        }
        assert!("unknown".parse::<AttendanceStatus>().is_err());
    }

    #[test]
    fn new_meeting_validates_and_deduplicates_units() {
        let u1 = new_id();
        let u2 = new_id();
        let presenter = new_id();
        let req: NewCourseMeeting = serde_json::from_value(json!({
            "title": "  Week 1 Study  ",
            "startsAt": 1_800_000_000_000_i64,
            "location": "  https://meet.example/room  ",
            "unitIds": [u1, u2, u1],
            "presenterId": presenter,
            "notes": "line 1\n\tline 2"
        }))
        .expect("valid json");
        let checked = req.check_request().expect("valid request");
        assert_eq!(checked.title, "Week 1 Study");
        assert_eq!(checked.location, "https://meet.example/room");
        assert_eq!(checked.unit_ids, vec![u1, u2]);
        assert_eq!(checked.presenter_id, Some(presenter));
        assert_eq!(checked.host_id, None);
        assert_eq!(checked.notes, "line 1\n\tline 2");
    }

    #[test]
    fn new_meeting_refuses_invalid_fields() {
        let req: NewCourseMeeting = serde_json::from_value(json!({
            "title": "   ",
            "startsAt": -1,
            "location": "a\nb",
            "unitIds": ["not-a-uuid"],
            "presenterId": "bad",
            "hostId": "",
            "notes": "bad\0note"
        }))
        .expect("valid json");
        let refused = req.check_request().expect_err("refused");
        let pairs: Vec<_> = refused
            .iter()
            .map(|r| (r.pointer.as_str(), r.reason))
            .collect();
        assert_eq!(
            pairs,
            [
                ("/title", RefusalReason::Required),
                ("/startsAt", RefusalReason::Invalid),
                ("/location", RefusalReason::InvalidCharacters),
                ("/unitIds/0", RefusalReason::InvalidFormat),
                ("/presenterId", RefusalReason::InvalidFormat),
                ("/hostId", RefusalReason::InvalidFormat),
                ("/notes", RefusalReason::InvalidCharacters),
            ]
        );
    }

    #[test]
    fn patch_distinguishes_omitted_null_and_set() {
        let host = new_id();
        let patch: CourseMeetingPatch = serde_json::from_value(json!({
            "presenterId": null,
            "hostId": host,
        }))
        .expect("valid patch");
        let checked = patch.check_request().expect("checked");
        assert_eq!(checked.presenter_id, OptionalIdChange::Clear);
        assert_eq!(checked.host_id, OptionalIdChange::Set(host));
        assert_eq!(checked.title, None);

        assert!(serde_json::from_value::<CourseMeetingPatch>(json!({"title": null})).is_err());
        assert!(serde_json::from_value::<CourseMeetingPatch>(json!({"startsAt": null})).is_err());
    }
}
