//! Personal unit progress and course-level completion summaries.
//!
//! A learner's own progress on each unit of a course records whether and when they
//! completed it, an optional planned date reserved for personal scheduling, and an
//! optional last read/watched position. Other members of the same course never see these
//! per-unit rows; they only see aggregate completed counts per active member in
//! [`CourseProgressSummary`].

use serde::{Deserialize, Deserializer, Serialize};

use crate::refusal::{RefusalReason, Refusals};

/// Maximum length in Unicode scalar values of [`UnitProgress::last_position`].
///
/// Covers scroll anchors, section headings, or media timestamps while keeping rows
/// bounded.
pub const UNIT_PROGRESS_POSITION_MAX_CHARS: usize = 256;

const POSITION_TOO_LONG: &str = "at most 256 characters";
const POSITION_CONTROL_CHARS: &str = "control characters are not allowed";

/// One learner's personal progress on a single unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnitProgress {
    /// Identifier of the unit block.
    pub unit_id: String,
    /// Completion time in milliseconds since the Unix epoch; `null` if not completed.
    pub completed_at: Option<i64>,
    /// Optional personal target date (`YYYY-MM-DD`), reserved for personal scheduling.
    pub planned_date: Option<String>,
    /// Optional last read or watched position string, at most 256 characters.
    pub last_position: Option<String>,
    /// Last modification time in milliseconds since the Unix epoch.
    pub updated_at: i64,
    /// Suggested next spaced-repetition review interval in days when the unit is
    /// completed; omitted when incomplete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_next_interval_days: Option<u32>,
}

/// How an update changes a unit's completion status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompletionChange {
    /// Leave the stored completion timestamp as it is (`None` on a new row).
    #[default]
    Keep,
    /// Mark the unit completed, preserving any existing completion timestamp.
    Complete,
    /// Clear the completion timestamp back to incomplete.
    Incomplete,
}

/// How an update changes [`UnitProgress::last_position`].
///
/// In JSON, omitting `lastPosition` keeps the existing value, `null` clears it, and a
/// string replaces it (after trimming; a blank string clears it).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LastPositionChange {
    /// Leave the stored position unchanged (`None` on a new row).
    #[default]
    Keep,
    /// Clear the stored position to `None`.
    Clear,
    /// Store a new position string.
    Set(String),
}

impl Serialize for LastPositionChange {
    /// Serializes [`LastPositionChange::Set`] as its string and both `Keep` and `Clear`
    /// as `null` (lossy: used so `utoipa` can emit schema defaults).
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Keep | Self::Clear => serializer.serialize_none(),
            Self::Set(position) => position.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for LastPositionChange {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Option::<String>::deserialize(deserializer)?.map_or(Self::Clear, Self::Set))
    }
}

/// Deserializes an explicit boolean while refusing `null` (omitted fields use `Default`).
fn deserialize_non_null_bool<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    bool::deserialize(deserializer).map(Some)
}

/// Request body for updating the caller's personal progress on a unit.
///
/// Omitted members leave the stored value untouched. Sending `completed: true` on an
/// already completed unit preserves its original `completedAt` timestamp. Sending
/// `lastPosition: null` (or a whitespace-only string) clears the stored position.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct UnitProgressUpdate {
    /// Set `true` to mark completed (idempotent), `false` to mark incomplete, or omit to
    /// leave unchanged (`null` is refused).
    #[serde(default, deserialize_with = "deserialize_non_null_bool")]
    #[schema(value_type = bool)]
    pub completed: Option<bool>,
    /// Last read or watched position (at most 256 characters, no control characters).
    /// Omit to keep unchanged, or send `null` to clear.
    #[schema(value_type = Option<String>)]
    pub last_position: LastPositionChange,
}

/// Validated and normalized update ready for storage.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnitProgressChange {
    /// How completion status changes.
    pub completion: CompletionChange,
    /// How the last position changes, already trimmed and validated.
    pub last_position: LastPositionChange,
}

impl UnitProgressUpdate {
    /// Validates and normalizes the request body into a [`UnitProgressChange`].
    ///
    /// # Errors
    ///
    /// Returns [`Refusals`] naming `/lastPosition` if it contains control characters or
    /// exceeds [`UNIT_PROGRESS_POSITION_MAX_CHARS`] characters after trimming.
    pub fn check_request(&self) -> Result<UnitProgressChange, Refusals> {
        let mut refused = Refusals::new();
        let last_position = match &self.last_position {
            LastPositionChange::Keep => LastPositionChange::Keep,
            LastPositionChange::Clear => LastPositionChange::Clear,
            LastPositionChange::Set(raw) => {
                if raw.chars().any(char::is_control) {
                    refused.refuse(
                        "lastPosition",
                        RefusalReason::InvalidCharacters,
                        POSITION_CONTROL_CHARS,
                    );
                    LastPositionChange::Keep
                } else {
                    let trimmed = raw.trim();
                    if trimmed.chars().count() > UNIT_PROGRESS_POSITION_MAX_CHARS {
                        refused.refuse("lastPosition", RefusalReason::TooLong, POSITION_TOO_LONG);
                        LastPositionChange::Keep
                    } else if trimmed.is_empty() {
                        LastPositionChange::Clear
                    } else {
                        LastPositionChange::Set(trimmed.to_owned())
                    }
                }
            }
        };
        refused.into_result()?;
        let completion = match self.completed {
            None => CompletionChange::Keep,
            Some(true) => CompletionChange::Complete,
            Some(false) => CompletionChange::Incomplete,
        };
        Ok(UnitProgressChange {
            completion,
            last_position,
        })
    }
}

/// Aggregate completion count of one active course member.
///
/// Intentionally omits unit identifiers, completion timestamps, planned dates, and last
/// positions so personal learning details are never disclosed to other members.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemberCourseProgress {
    /// Identifier of the active course member.
    pub user_id: String,
    /// Number of currently live units in the course completed by this member.
    pub completed_units: u32,
}

/// Course-wide progress summary across all active members.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CourseProgressSummary {
    /// Total number of currently live units in the course. Chapters and trashed units are
    /// excluded.
    pub total_units: u32,
    /// Per-member completion counts for active course members, ordered by `userId`.
    pub members: Vec<MemberCourseProgress>,
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn decode(value: Value) -> UnitProgressUpdate {
        serde_json::from_value(value).expect("valid update json")
    }

    #[test]
    fn update_distinguishes_omitted_null_and_set_last_position() {
        assert_eq!(
            decode(json!({})).check_request(),
            Ok(UnitProgressChange {
                completion: CompletionChange::Keep,
                last_position: LastPositionChange::Keep,
            })
        );
        assert_eq!(
            decode(json!({"completed": true, "lastPosition": null})).check_request(),
            Ok(UnitProgressChange {
                completion: CompletionChange::Complete,
                last_position: LastPositionChange::Clear,
            })
        );
        assert_eq!(
            decode(json!({"completed": false, "lastPosition": "  section-2  "})).check_request(),
            Ok(UnitProgressChange {
                completion: CompletionChange::Incomplete,
                last_position: LastPositionChange::Set("section-2".to_owned()),
            })
        );
        assert_eq!(
            decode(json!({"lastPosition": "   "})).check_request(),
            Ok(UnitProgressChange {
                completion: CompletionChange::Keep,
                last_position: LastPositionChange::Clear,
            })
        );
    }

    #[test]
    fn update_refuses_unknown_fields_control_chars_and_excess_length() {
        assert!(
            serde_json::from_value::<UnitProgressUpdate>(json!({"plannedDate": "2026-11-23"}))
                .is_err()
        );
        assert!(serde_json::from_value::<UnitProgressUpdate>(json!({"completed": null})).is_err());

        for bad in [
            "a\nb",
            "a\tb",
            "a\0b",
            "\x1b[31m",
            "\r",
            " \n ",
            "a\u{0085}b",
            "a\u{009b}b",
        ] {
            let refused = decode(json!({"lastPosition": bad}))
                .check_request()
                .expect_err("control char refused");
            let pairs: Vec<_> = refused
                .iter()
                .map(|r| (r.pointer.as_str(), r.reason))
                .collect();
            assert_eq!(pairs, [("/lastPosition", RefusalReason::InvalidCharacters)]);
        }

        let exact = "가".repeat(UNIT_PROGRESS_POSITION_MAX_CHARS);
        assert_eq!(
            decode(json!({"lastPosition": format!("  {exact}  ")})).check_request(),
            Ok(UnitProgressChange {
                completion: CompletionChange::Keep,
                last_position: LastPositionChange::Set(exact),
            })
        );

        let too_long = "가".repeat(UNIT_PROGRESS_POSITION_MAX_CHARS + 1);
        let refused = decode(json!({"lastPosition": too_long}))
            .check_request()
            .expect_err("too long refused");
        let pairs: Vec<_> = refused
            .iter()
            .map(|r| (r.pointer.as_str(), r.reason))
            .collect();
        assert_eq!(pairs, [("/lastPosition", RefusalReason::TooLong)]);
    }
}
