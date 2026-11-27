//! Course exams and strictly personal exam scores.
//!
//! Exam definitions (`CourseExam`) are shared within a course, while individual scores
//! (`MyExamScore`) belong strictly to the learner who recorded them. Other course members
//! never receive another member's score or note—only the aggregate `averageScore` and
//! `scoreCount` across active enrolled course members.

use serde::{Deserialize, Deserializer, Serialize};

use crate::diary::DiaryDate;
use crate::refusal::{RefusalReason, Refusals};

/// Maximum length in Unicode scalar values of [`CourseExam::title`].
pub const COURSE_EXAM_TITLE_MAX_CHARS: usize = 200;

/// Maximum length in Unicode scalar values of [`CourseExam::description`].
pub const COURSE_EXAM_DESCRIPTION_MAX_CHARS: usize = 5000;

/// Maximum length in Unicode scalar values of [`MyExamScore::note`].
pub const EXAM_SCORE_NOTE_MAX_CHARS: usize = 1000;

/// Maximum permitted [`CourseExam::max_score`], keeping SQL `AVG` / `SUM` far below
/// `f64::MAX` overflow even across large courses.
pub const COURSE_EXAM_MAX_SCORE_LIMIT: f64 = 1_000_000_000.0;

const TITLE_REQUIRED: &str = "exam title is required";
const TITLE_TOO_LONG: &str = "at most 200 characters";
const DESCRIPTION_TOO_LONG: &str = "at most 5000 characters";
const NOTE_TOO_LONG: &str = "at most 1000 characters";
const CONTROL_CHARS: &str = "control characters are not allowed";
const INVALID_DATE: &str = "must be a valid calendar date (YYYY-MM-DD)";
const INVALID_MAX_SCORE: &str = "maxScore must be greater than 0 and at most 1000000000";
const INVALID_SCORE: &str = "score must be a finite number greater than or equal to 0";

/// The calling learner's own personal score on an exam.
///
/// Intentionally carries no user identifier and is never embedded for any user other than
/// the authenticated caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MyExamScore {
    /// Score achieved by the caller (`0 <= score <= maxScore`).
    pub score: f64,
    /// Personal reflection note, at most 1000 characters; empty if unset.
    pub note: String,
    /// Last modification time in milliseconds since the Unix epoch.
    pub updated_at: i64,
}

/// A course exam with the caller's own score (if any) and aggregate statistics across
/// active enrolled course members.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CourseExam {
    /// Identifier of the exam.
    pub id: String,
    /// Identifier of the course container.
    pub container_id: String,
    /// Exam title, at most 200 characters.
    pub title: String,
    /// Optional calendar date of the exam (`YYYY-MM-DD`).
    pub exam_date: Option<String>,
    /// Maximum possible score (strictly positive).
    pub max_score: f64,
    /// Shared exam description, at most 5000 characters; empty if unset.
    pub description: String,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Last modification time in milliseconds since the Unix epoch.
    pub updated_at: i64,
    /// The caller's own score row, or `null` if none (never another member's score).
    pub my_score: Option<MyExamScore>,
    /// Published snapshot mean score (rounded to 0.1) across active enrolled course members
    /// once a course administrator publishes exam results (requiring at least 5 active
    /// scorers); `null` before publication or for unjoined readers.
    pub average_score: Option<f64>,
    /// Number of active enrolled course members who recorded a score (`0` for unjoined
    /// readers).
    pub score_count: u32,
    /// When the exam result snapshot was published by a course administrator, in
    /// milliseconds since the Unix epoch; `null` while unpublished or for unjoined readers.
    #[serde(default)]
    pub results_published_at: Option<i64>,
    /// Published active-scorer count rounded down to a multiple of 5 (`count / 5 * 5`), or
    /// `null` while unpublished or for unjoined readers.
    #[serde(default)]
    pub published_score_bucket: Option<u32>,
}

/// How a patch changes [`CourseExam::exam_date`].
///
/// Omitting `examDate` keeps the existing value, `null` clears it, and a `YYYY-MM-DD`
/// string replaces it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ExamDateChange {
    /// Leave the stored date unchanged.
    #[default]
    Keep,
    /// Clear the stored date to `None`.
    Clear,
    /// Replace the stored date.
    Set(String),
}

impl Serialize for ExamDateChange {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Keep | Self::Clear => serializer.serialize_none(),
            Self::Set(date) => date.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ExamDateChange {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Option::<String>::deserialize(deserializer)?.map_or(Self::Clear, Self::Set))
    }
}

fn deserialize_non_null_string<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

fn deserialize_non_null_f64<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<f64>, D::Error> {
    f64::deserialize(deserializer).map(Some)
}

/// Request body for creating a course exam.
#[derive(Debug, Clone, PartialEq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NewCourseExam {
    /// Exam title (required, 1–200 characters, no control characters).
    pub title: String,
    /// Optional calendar date (`YYYY-MM-DD`).
    #[serde(default)]
    pub exam_date: Option<String>,
    /// Maximum possible score (finite number strictly greater than 0).
    pub max_score: f64,
    /// Shared exam description (at most 5000 characters).
    #[serde(default)]
    pub description: String,
}

/// Validated new course exam ready for storage.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedNewExam {
    /// Trimmed, validated title.
    pub title: String,
    /// Optional validated calendar date (`YYYY-MM-DD`).
    pub exam_date: Option<String>,
    /// Validated strictly positive maximum score.
    pub max_score: f64,
    /// Validated description.
    pub description: String,
}

impl NewCourseExam {
    /// Validates and normalizes the request body into a [`ValidatedNewExam`].
    ///
    /// # Errors
    ///
    /// Returns [`Refusals`] naming every invalid field.
    pub fn check_request(&self) -> Result<ValidatedNewExam, Refusals> {
        let mut refused = Refusals::new();
        let title = check_title(&self.title, &mut refused);
        let exam_date = check_optional_date(self.exam_date.as_deref(), &mut refused);
        check_max_score(self.max_score, &mut refused);
        let description = check_multiline(
            "description",
            &self.description,
            COURSE_EXAM_DESCRIPTION_MAX_CHARS,
            DESCRIPTION_TOO_LONG,
            &mut refused,
        );
        refused.into_result()?;
        Ok(ValidatedNewExam {
            title,
            exam_date,
            max_score: self.max_score,
            description,
        })
    }
}

/// Request body for patching a course exam.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CourseExamPatch {
    /// Replacement title (1–200 characters, no control characters).
    #[serde(default, deserialize_with = "deserialize_non_null_string")]
    #[schema(value_type = String)]
    pub title: Option<String>,
    /// Replacement exam date (`YYYY-MM-DD`); `null` clears it.
    #[schema(value_type = Option<String>)]
    pub exam_date: ExamDateChange,
    /// Replacement maximum score (finite number strictly greater than 0).
    #[serde(default, deserialize_with = "deserialize_non_null_f64")]
    #[schema(value_type = f64)]
    pub max_score: Option<f64>,
    /// Replacement description (at most 5000 characters).
    #[serde(default, deserialize_with = "deserialize_non_null_string")]
    #[schema(value_type = String)]
    pub description: Option<String>,
}

/// Validated exam patch ready for storage.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CourseExamChange {
    /// Replacement title, if changing.
    pub title: Option<String>,
    /// Replacement exam date change.
    pub exam_date: ExamDateChange,
    /// Replacement maximum score, if changing.
    pub max_score: Option<f64>,
    /// Replacement description, if changing.
    pub description: Option<String>,
}

impl CourseExamPatch {
    /// Validates and normalizes the patch into a [`CourseExamChange`].
    ///
    /// # Errors
    ///
    /// Returns [`Refusals`] naming every invalid field.
    pub fn check_request(&self) -> Result<CourseExamChange, Refusals> {
        let mut refused = Refusals::new();
        let title = self
            .title
            .as_deref()
            .map(|raw| check_title(raw, &mut refused));
        let exam_date = match &self.exam_date {
            ExamDateChange::Keep => ExamDateChange::Keep,
            ExamDateChange::Clear => ExamDateChange::Clear,
            ExamDateChange::Set(raw) => {
                if raw.parse::<DiaryDate>().is_err() {
                    refused.refuse("examDate", RefusalReason::InvalidFormat, INVALID_DATE);
                    ExamDateChange::Keep
                } else {
                    ExamDateChange::Set(raw.clone())
                }
            }
        };
        if let Some(max_score) = self.max_score {
            check_max_score(max_score, &mut refused);
        }
        let description = self.description.as_deref().map(|raw| {
            check_multiline(
                "description",
                raw,
                COURSE_EXAM_DESCRIPTION_MAX_CHARS,
                DESCRIPTION_TOO_LONG,
                &mut refused,
            )
        });
        refused.into_result()?;
        Ok(CourseExamChange {
            title,
            exam_date,
            max_score: self.max_score,
            description,
        })
    }
}

/// Request body for recording or updating the caller's own exam score.
#[derive(Debug, Clone, PartialEq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MyExamScoreUpdate {
    /// Caller's score (`0 <= score <= exam.maxScore`).
    pub score: f64,
    /// Optional personal note (at most 1000 characters).
    #[serde(default)]
    pub note: String,
}

/// Validated personal exam score ready for storage (upper bound against `exam.max_score`
/// is checked atomically when writing).
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedMyExamScore {
    /// Finite score >= 0.
    pub score: f64,
    /// Validated personal note.
    pub note: String,
}

impl MyExamScoreUpdate {
    /// Validates the request body into a [`ValidatedMyExamScore`].
    ///
    /// # Errors
    ///
    /// Returns [`Refusals`] naming `/score` or `/note` when invalid.
    pub fn check_request(&self) -> Result<ValidatedMyExamScore, Refusals> {
        let mut refused = Refusals::new();
        if !self.score.is_finite() || self.score < 0.0 {
            refused.refuse("score", RefusalReason::Invalid, INVALID_SCORE);
        }
        let note = check_multiline(
            "note",
            &self.note,
            EXAM_SCORE_NOTE_MAX_CHARS,
            NOTE_TOO_LONG,
            &mut refused,
        );
        refused.into_result()?;
        let score = if self.score == 0.0 { 0.0 } else { self.score };
        Ok(ValidatedMyExamScore { score, note })
    }
}

fn check_title(raw: &str, refused: &mut Refusals) -> String {
    if raw.chars().any(char::is_control) {
        refused.refuse("title", RefusalReason::InvalidCharacters, CONTROL_CHARS);
        return String::new();
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        refused.refuse("title", RefusalReason::Required, TITLE_REQUIRED);
    } else if trimmed.chars().count() > COURSE_EXAM_TITLE_MAX_CHARS {
        refused.refuse("title", RefusalReason::TooLong, TITLE_TOO_LONG);
    }
    trimmed.to_owned()
}

fn check_optional_date(raw: Option<&str>, refused: &mut Refusals) -> Option<String> {
    let date = raw?;
    if date.parse::<DiaryDate>().is_err() {
        refused.refuse("examDate", RefusalReason::InvalidFormat, INVALID_DATE);
        return None;
    }
    Some(date.to_owned())
}

fn check_max_score(max_score: f64, refused: &mut Refusals) {
    if !max_score.is_finite() || max_score <= 0.0 || max_score > COURSE_EXAM_MAX_SCORE_LIMIT {
        refused.refuse("maxScore", RefusalReason::Invalid, INVALID_MAX_SCORE);
    }
}

fn check_multiline(
    field: &str,
    raw: &str,
    max_chars: usize,
    too_long: &str,
    refused: &mut Refusals,
) -> String {
    if raw
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        refused.refuse(field, RefusalReason::InvalidCharacters, CONTROL_CHARS);
        return String::new();
    }
    if raw.chars().count() > max_chars {
        refused.refuse(field, RefusalReason::TooLong, too_long);
    }
    raw.to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn new_exam_validates_fields() {
        let req: NewCourseExam = serde_json::from_value(json!({
            "title": "  Midterm  ",
            "examDate": "2026-12-01",
            "maxScore": 100.0,
            "description": "Chapters 1-5\nOpen book"
        }))
        .expect("valid json");
        let checked = req.check_request().expect("valid");
        assert_eq!(checked.title, "Midterm");
        assert_eq!(checked.exam_date.as_deref(), Some("2026-12-01"));
        assert_eq!(checked.max_score, 100.0);
        assert_eq!(checked.description, "Chapters 1-5\nOpen book");
    }

    #[test]
    fn new_exam_refuses_bad_values() {
        let req: NewCourseExam = serde_json::from_value(json!({
            "title": "",
            "examDate": "2026-02-30",
            "maxScore": 0.0,
            "description": "a\0b"
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
                ("/examDate", RefusalReason::InvalidFormat),
                ("/maxScore", RefusalReason::Invalid),
                ("/description", RefusalReason::InvalidCharacters),
            ]
        );
    }

    #[test]
    fn my_score_update_validates_bounds_and_note() {
        let ok: MyExamScoreUpdate =
            serde_json::from_value(json!({"score": -0.0, "note": "good\njob"})).expect("json");
        let checked = ok.check_request().expect("valid zero");
        assert!(checked.score.is_sign_positive() && checked.score == 0.0);

        let huge_exam: NewCourseExam = serde_json::from_value(json!({
            "title": "Overflow Attempt",
            "maxScore": 1e308
        }))
        .expect("json");
        assert!(huge_exam.check_request().is_err());

        let bad: MyExamScoreUpdate = serde_json::from_value(json!({
            "score": -0.5,
            "note": "x".repeat(EXAM_SCORE_NOTE_MAX_CHARS + 1)
        }))
        .expect("json");
        let refused = bad.check_request().expect_err("refused");
        let pairs: Vec<_> = refused
            .iter()
            .map(|r| (r.pointer.as_str(), r.reason))
            .collect();
        assert_eq!(
            pairs,
            [
                ("/score", RefusalReason::Invalid),
                ("/note", RefusalReason::TooLong),
            ]
        );
    }

    #[test]
    fn exam_patch_handles_tristate_date_and_rejects_null_non_nullables() {
        let patch: CourseExamPatch = serde_json::from_value(json!({
            "examDate": null,
            "maxScore": 50.0
        }))
        .expect("valid patch");
        let checked = patch.check_request().expect("checked");
        assert_eq!(checked.exam_date, ExamDateChange::Clear);
        assert_eq!(checked.max_score, Some(50.0));
        assert!(serde_json::from_value::<CourseExamPatch>(json!({"maxScore": null})).is_err());
        assert!(serde_json::from_value::<CourseExamPatch>(json!({"title": null})).is_err());
    }
}
