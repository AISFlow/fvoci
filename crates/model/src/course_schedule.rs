//! Course schedules, unit study amounts, proportional distribution, and today/overdue views.
//!
//! A course may keep one shared schedule (`startDate`, `targetDate`, `studyDays`) and
//! each unit (`BlockType::Unit`) may carry an optional study `amount` (`1..=100_000`),
//! `amountUnit` (`minutes`, `pages`, `problems`), and shared `scheduledDate`
//! (`YYYY-MM-DD`) in `Block.fields`. Learners can also override their own personal
//! `plannedDate` in `unit_progress`, visible only to themselves; `effectiveDate` resolves
//! as `myPlannedDate.or(scheduledDate)`.
//!
//! Overdue incomplete units are never automatically shifted forward as time passes: they
//! stay on their scheduled dates and surface as `overdue` until completed or manually
//! redistributed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::JsonMap;
use crate::date_value::day_at;
use crate::diary::DiaryDate;
use crate::refusal::{RefusalReason, Refusals};
use crate::settings::TimeZone;
use crate::unit::{UnitKind, UnitNode};
use crate::unit_progress::UnitProgress;

/// The field of a unit block that holds its study amount (`1..=100_000`).
pub const UNIT_AMOUNT_FIELD: &str = "amount";

/// The field of a unit block that holds the unit of its study amount (`AmountUnit`).
pub const UNIT_AMOUNT_UNIT_FIELD: &str = "amountUnit";

/// The field of a unit block that holds its shared scheduled date (`YYYY-MM-DD`).
pub const UNIT_SCHEDULED_DATE_FIELD: &str = "scheduledDate";

/// Minimum study amount a unit may carry when set.
pub const UNIT_AMOUNT_MIN: u32 = 1;

/// Maximum study amount a unit may carry when set.
pub const UNIT_AMOUNT_MAX: u32 = 100_000;

/// Maximum span in days between a schedule's start date and target date (10 years).
///
/// Keeps date-range iteration bounded while accommodating multi-year curricula.
pub const MAX_SCHEDULE_SPAN_DAYS: i64 = 3_660;

/// Study-days bitmask bit for Monday (bit 0).
pub const STUDY_DAYS_MONDAY: u8 = 1 << 0;
/// Study-days bitmask bit for Tuesday (bit 1).
pub const STUDY_DAYS_TUESDAY: u8 = 1 << 1;
/// Study-days bitmask bit for Wednesday (bit 2).
pub const STUDY_DAYS_WEDNESDAY: u8 = 1 << 2;
/// Study-days bitmask bit for Thursday (bit 3).
pub const STUDY_DAYS_THURSDAY: u8 = 1 << 3;
/// Study-days bitmask bit for Friday (bit 4).
pub const STUDY_DAYS_FRIDAY: u8 = 1 << 4;
/// Study-days bitmask bit for Saturday (bit 5).
pub const STUDY_DAYS_SATURDAY: u8 = 1 << 5;
/// Study-days bitmask bit for Sunday (bit 6).
pub const STUDY_DAYS_SUNDAY: u8 = 1 << 6;
/// Study-days bitmask covering Monday through Friday (`31`).
pub const STUDY_DAYS_WEEKDAYS: u8 = 0b001_1111;
/// Study-days bitmask covering every day of the week (`127`).
pub const STUDY_DAYS_ALL: u8 = 0b111_1111;

const DATE_INVALID: &str = "a date must be a valid calendar day written as YYYY-MM-DD";
const AMOUNT_OUT_OF_RANGE: &str = "amount must be an integer from 1 to 100000";
const STUDY_DAYS_INVALID: &str = "studyDays must be a weekday bitmask from 1 to 127";
const TARGET_BEFORE_START: &str = "targetDate must be on or after the start date";
const SPAN_TOO_LONG: &str = "schedule span must be at most 3660 days";
const NO_STUDY_DAYS_IN_RANGE: &str =
    "no selected study day falls between the start date and targetDate";
const TARGET_REQUIRED: &str = "targetDate is required when the course has no saved schedule";
const STUDY_DAYS_REQUIRED: &str = "studyDays is required when the course has no saved schedule";

/// Unit of measure for a course unit's study amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AmountUnit {
    /// Study duration in minutes.
    Minutes,
    /// Study volume in pages.
    Pages,
    /// Study volume in problems or exercises.
    Problems,
}

impl AmountUnit {
    /// Every supported unit of study amount.
    pub const ALL: [Self; 3] = [Self::Minutes, Self::Pages, Self::Problems];

    /// Returns the wire and storage string form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minutes => "minutes",
            Self::Pages => "pages",
            Self::Problems => "problems",
        }
    }
}

impl fmt::Display for AmountUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when text does not name an [`AmountUnit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("amountUnit must be one of minutes, pages, or problems")]
pub struct ParseAmountUnitError;

impl FromStr for AmountUnit {
    type Err = ParseAmountUnitError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|unit| unit.as_str() == text)
            .ok_or(ParseAmountUnitError)
    }
}

/// Non-empty set of study weekdays encoded as a 7-bit mask (`1..=127`, Mon=1 .. Sun=64).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StudyDays(u8);

impl StudyDays {
    /// Creates a [`StudyDays`] bitmask if `bits` lies in `1..=127`.
    #[must_use]
    pub const fn try_new(bits: u8) -> Option<Self> {
        if bits >= 1 && bits <= STUDY_DAYS_ALL {
            Some(Self(bits))
        } else {
            None
        }
    }

    /// Returns the raw bitmask (`1..=127`).
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Returns whether `date` falls on a selected study weekday.
    #[must_use]
    pub fn includes_date(self, date: DiaryDate) -> bool {
        let bit = match date.civil_date().weekday() {
            jiff::civil::Weekday::Monday => STUDY_DAYS_MONDAY,
            jiff::civil::Weekday::Tuesday => STUDY_DAYS_TUESDAY,
            jiff::civil::Weekday::Wednesday => STUDY_DAYS_WEDNESDAY,
            jiff::civil::Weekday::Thursday => STUDY_DAYS_THURSDAY,
            jiff::civil::Weekday::Friday => STUDY_DAYS_FRIDAY,
            jiff::civil::Weekday::Saturday => STUDY_DAYS_SATURDAY,
            jiff::civil::Weekday::Sunday => STUDY_DAYS_SUNDAY,
        };
        (self.0 & bit) != 0
    }
}

/// Tri-state patch change for an optional field: keep existing, clear (`null`), or set.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum FieldChange<T> {
    /// Leave the existing value untouched.
    #[default]
    Keep,
    /// Clear the stored value (`null` in JSON).
    Clear,
    /// Replace the stored value.
    Set(T),
}

impl<T> FieldChange<T> {
    /// Returns `true` when this change leaves the stored field untouched.
    #[must_use]
    pub const fn is_keep(&self) -> bool {
        matches!(self, Self::Keep)
    }
}

/// Serializing `Keep` produces JSON `null` (lossy against `Clear`); enclosing patch
/// structs MUST annotate `#[serde(skip_serializing_if = "FieldChange::is_keep")]` so
/// omitted and cleared fields stay distinct on the wire.
impl<T: Serialize> Serialize for FieldChange<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Keep | Self::Clear => serializer.serialize_none(),
            Self::Set(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for FieldChange<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Option::<T>::deserialize(deserializer)?.map_or(Self::Clear, Self::Set))
    }
}

/// Whether `value` is a valid stored unit amount (`1..=100_000`).
pub(crate) fn is_unit_amount_value(value: &Value) -> bool {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .is_some_and(|n| (UNIT_AMOUNT_MIN..=UNIT_AMOUNT_MAX).contains(&n))
}

/// Whether `value` is a valid stored [`AmountUnit`] string.
pub(crate) fn is_amount_unit_value(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| text.parse::<AmountUnit>().is_ok())
}

/// Reads the optional study amount from a block's `fields`.
#[must_use]
pub(crate) fn unit_amount(fields: &JsonMap) -> Option<u32> {
    fields
        .get(UNIT_AMOUNT_FIELD)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| (UNIT_AMOUNT_MIN..=UNIT_AMOUNT_MAX).contains(n))
}

/// Reads the optional [`AmountUnit`] from a block's `fields`.
#[must_use]
pub(crate) fn unit_amount_unit(fields: &JsonMap) -> Option<AmountUnit> {
    fields
        .get(UNIT_AMOUNT_UNIT_FIELD)
        .and_then(Value::as_str)
        .and_then(|text| text.parse().ok())
}

/// Reads the optional shared `scheduledDate` (`YYYY-MM-DD`) from a block's `fields`.
#[must_use]
pub(crate) fn unit_scheduled_date(fields: &JsonMap) -> Option<String> {
    fields
        .get(UNIT_SCHEDULED_DATE_FIELD)
        .and_then(Value::as_str)
        .filter(|text| text.parse::<DiaryDate>().is_ok())
        .map(str::to_owned)
}

/// Validates an optional `amount` value (`1..=100_000`), recording a refusal at `/amount`.
pub(crate) fn refuse_unit_amount(refused: &mut Refusals, amount: Option<i64>) {
    if let Some(value) = amount {
        let min = i64::from(UNIT_AMOUNT_MIN);
        let max = i64::from(UNIT_AMOUNT_MAX);
        if !(min..=max).contains(&value) {
            refused.refuse("amount", RefusalReason::Invalid, AMOUNT_OUT_OF_RANGE);
        }
    }
}

/// Validates an optional `scheduledDate` string as a calendar date (`YYYY-MM-DD`).
pub(crate) fn refuse_unit_scheduled_date(refused: &mut Refusals, date: Option<&str>) {
    if let Some(text) = date
        && text.parse::<DiaryDate>().is_err()
    {
        refused.refuse("scheduledDate", RefusalReason::InvalidFormat, DATE_INVALID);
    }
}

/// Stored shared study schedule of a course.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CourseSchedule {
    /// Start date of the study period (`YYYY-MM-DD`).
    pub start_date: String,
    /// Target completion date of the study period (`YYYY-MM-DD`).
    pub target_date: String,
    /// Weekday bitmask (`1..=127`; bit 0 = Monday .. bit 6 = Sunday).
    #[schema(minimum = 1, maximum = 127)]
    pub study_days: u8,
    /// Last modification time in milliseconds since the Unix epoch.
    pub updated_at: i64,
}

/// Request body for creating or updating a course's shared schedule.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CourseScheduleUpdate {
    /// Start date of the study period (`YYYY-MM-DD`).
    pub start_date: String,
    /// Target completion date (`YYYY-MM-DD`), on or after `startDate`.
    pub target_date: String,
    /// Weekday bitmask (`1..=127`; bit 0 = Monday .. bit 6 = Sunday).
    #[schema(value_type = u8, minimum = 1, maximum = 127)]
    pub study_days: i32,
    /// When `true`, immediately assigns shared `scheduledDate` values across all live
    /// units proportionally by study amount. Left out, `false`.
    #[serde(default)]
    pub auto_assign: bool,
}

/// Validated course schedule ready for persistence and optional auto-assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidatedCourseSchedule {
    /// Start date of the study period.
    pub start_date: DiaryDate,
    /// Target end date of the study period.
    pub target_date: DiaryDate,
    /// Validated weekday bitmask.
    pub study_days: StudyDays,
    /// Whether to immediately distribute all live units across the period.
    pub auto_assign: bool,
}

/// Property key carrying a built-in course template's example shared schedule until copied.
pub const TEMPLATE_SCHEDULE_PROPERTY: &str = "templateSchedule";

impl ValidatedCourseSchedule {
    /// Shifts `start_date` and `target_date` by `days` calendar days (preserving span
    /// and `study_days`), returning `None` if shifting leaves calendar bounds or produces
    /// a window with no matching study day.
    #[must_use]
    pub fn try_shifted(self, days: i64) -> Option<Self> {
        if days == 0 {
            return Some(self);
        }
        let delta = i32::try_from(days).ok()?;
        let start_date = self.start_date.plus_days(delta)?;
        let target_date = self.target_date.plus_days(delta)?;
        has_study_day_in_range(start_date, target_date, self.study_days).then_some(Self {
            start_date,
            target_date,
            ..self
        })
    }
}

impl CourseSchedule {
    /// Parses a stored [`CourseSchedule`] into a [`ValidatedCourseSchedule`] with
    /// `auto_assign: false`, returning `None` if dates or `study_days` are invalid.
    #[must_use]
    pub fn to_validated(&self) -> Option<ValidatedCourseSchedule> {
        let update = CourseScheduleUpdate {
            start_date: self.start_date.clone(),
            target_date: self.target_date.clone(),
            study_days: i32::from(self.study_days),
            auto_assign: false,
        };
        update.check_request().ok()
    }
}

/// Shifts a unit's shared `scheduledDate` field (`YYYY-MM-DD`) by `days` calendar days.
/// Leaves the field unchanged when `days == 0`, absent, malformed, or out of calendar bounds.
pub fn shift_unit_scheduled_date(fields: &mut JsonMap, days: i64) {
    if days == 0 {
        return;
    }
    let Ok(delta) = i32::try_from(days) else {
        return;
    };
    let Some(Value::String(raw)) = fields.get_mut(UNIT_SCHEDULED_DATE_FIELD) else {
        return;
    };
    if let Ok(date) = raw.parse::<DiaryDate>()
        && let Some(moved) = date.plus_days(delta)
    {
        *raw = moved.to_string();
    }
}

impl CourseScheduleUpdate {
    /// Validates the schedule dates, weekday mask, span limit, and non-empty study days.
    ///
    /// # Errors
    ///
    /// Returns [`Refusals`] naming every invalid field (`/startDate`, `/targetDate`,
    /// `/studyDays`).
    pub fn check_request(&self) -> Result<ValidatedCourseSchedule, Refusals> {
        let mut refused = Refusals::new();
        let start = parse_date_field(&mut refused, "startDate", &self.start_date);
        let target = parse_date_field(&mut refused, "targetDate", &self.target_date);
        let study_days = parse_study_days(&mut refused, self.study_days);
        validate_schedule_window(&mut refused, start, target, study_days);
        let (Some(start_date), Some(target_date), Some(study_days)) = (start, target, study_days)
        else {
            return Err(refused);
        };
        refused.into_result()?;
        Ok(ValidatedCourseSchedule {
            start_date,
            target_date,
            study_days,
            auto_assign: self.auto_assign,
        })
    }
}

/// Request body for manually redistributing a course's shared schedule or the caller's
/// personal schedule.
///
/// Omitted or `null` members fall back to defaults: `fromDate` defaults to today in the
/// caller's configured time zone (UTC when `auto`), and `targetDate` / `studyDays` fall
/// back to the course's saved schedule.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RedistributeScheduleRequest {
    /// Start date (`YYYY-MM-DD`) for redistribution; omitted or `null` uses caller's today.
    pub from_date: Option<String>,
    /// Target end date (`YYYY-MM-DD`); omitted or `null` uses the course's saved `targetDate`.
    pub target_date: Option<String>,
    /// Weekday bitmask (`1..=127`); omitted or `null` uses the course's saved `studyDays`.
    #[schema(value_type = Option<u8>, minimum = 1, maximum = 127)]
    pub study_days: Option<i32>,
}

/// Validated parameters for a manual redistribution pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedistributePlan {
    /// First day eligible to receive redistributed units.
    pub from_date: DiaryDate,
    /// Last day eligible to receive redistributed units.
    pub target_date: DiaryDate,
    /// Selected study weekdays.
    pub study_days: StudyDays,
}

impl RedistributeScheduleRequest {
    /// Resolves and validates the redistribution parameters against `today` (in the
    /// caller's time zone) and the course's optional stored schedule.
    ///
    /// # Errors
    ///
    /// Returns [`Refusals`] naming missing or invalid fields (`/fromDate`, `/targetDate`,
    /// `/studyDays`).
    pub fn resolve(
        &self,
        today: DiaryDate,
        stored: Option<&CourseSchedule>,
    ) -> Result<RedistributePlan, Refusals> {
        let mut refused = Refusals::new();
        let from = match &self.from_date {
            Some(text) => parse_date_field(&mut refused, "fromDate", text),
            None => Some(today),
        };
        let target = match &self.target_date {
            Some(text) => parse_date_field(&mut refused, "targetDate", text),
            None => {
                if let Some(date) = stored.and_then(|s| s.target_date.parse::<DiaryDate>().ok()) {
                    Some(date)
                } else {
                    refused.refuse("targetDate", RefusalReason::Required, TARGET_REQUIRED);
                    None
                }
            }
        };
        let study_days = match self.study_days {
            Some(bits) => parse_study_days(&mut refused, bits),
            None => {
                if let Some(days) = stored.and_then(|s| StudyDays::try_new(s.study_days)) {
                    Some(days)
                } else {
                    refused.refuse("studyDays", RefusalReason::Required, STUDY_DAYS_REQUIRED);
                    None
                }
            }
        };
        validate_schedule_window(&mut refused, from, target, study_days);
        let (Some(from_date), Some(target_date), Some(study_days)) = (from, target, study_days)
        else {
            return Err(refused);
        };
        refused.into_result()?;
        Ok(RedistributePlan {
            from_date,
            target_date,
            study_days,
        })
    }
}

fn deserialize_required_option<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

/// Request body for setting or clearing the caller's personal planned date on a unit.
///
/// The `plannedDate` member is required on the wire (`null` clears the personal override;
/// an omitted key `{}` is rejected with 400).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnitScheduleUpdate {
    /// Personal planned date (`YYYY-MM-DD`), or `null` to clear the personal override and
    /// revert to the unit's shared `scheduledDate`.
    #[serde(deserialize_with = "deserialize_required_option")]
    pub planned_date: Option<String>,
}

impl UnitScheduleUpdate {
    /// Validates `plannedDate` if present.
    ///
    /// # Errors
    ///
    /// Returns [`Refusals`] naming `/plannedDate` when the text is not a valid `YYYY-MM-DD`
    /// calendar date.
    pub fn check_request(&self) -> Result<Option<DiaryDate>, Refusals> {
        let Some(raw) = &self.planned_date else {
            return Ok(None);
        };
        let mut refused = Refusals::new();
        let parsed = parse_date_field(&mut refused, "plannedDate", raw);
        refused.into_result()?;
        Ok(parsed)
    }
}

/// One live unit of a course with its shared schedule, caller's personal override,
/// effective date, and completion/overdue status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledUnit {
    /// Identifier of the unit block.
    pub unit_id: String,
    /// Identifier of the chapter this unit sits under; empty for a top-level unit.
    pub parent_id: String,
    /// Title of the unit.
    pub title: String,
    /// Optional study amount (`1..=100_000`); treated as weight `1` when `null`.
    #[schema(minimum = 1, maximum = 100_000)]
    pub amount: Option<u32>,
    /// Optional unit of study amount (`minutes`, `pages`, or `problems`).
    pub amount_unit: Option<AmountUnit>,
    /// Shared scheduled date (`YYYY-MM-DD`) stored on the unit block, if any.
    pub scheduled_date: Option<String>,
    /// Caller's own personal planned date (`YYYY-MM-DD`), if any. Never shows another
    /// user's planned date.
    pub my_planned_date: Option<String>,
    /// Effective date for the caller: `myPlannedDate` if set, otherwise `scheduledDate`.
    pub effective_date: Option<String>,
    /// Whether the caller has completed this unit.
    pub completed: bool,
    /// Whether this unit is overdue for the caller (`!completed && effectiveDate < today`).
    pub overdue: bool,
}

/// Full schedule view of a course for the calling user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CourseScheduleView {
    /// The course's saved shared schedule, or `null` if none has been configured yet.
    pub schedule: Option<CourseSchedule>,
    /// Today's calendar date (`YYYY-MM-DD`) in the caller's configured time zone (UTC when
    /// set to `auto`).
    pub today: String,
    /// Number of incomplete live units whose `effectiveDate` equals `today`.
    pub today_count: u32,
    /// Number of incomplete live units whose `effectiveDate` is strictly before `today`.
    pub overdue_count: u32,
    /// Live units of the course in outline tree order (chapters and trashed units excluded).
    pub units: Vec<ScheduledUnit>,
}

/// One incomplete unit scheduled for today or overdue across enrolled courses in a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LearningTodayItem {
    /// Identifier of the course holding this unit.
    pub course_id: String,
    /// Title of the course holding this unit.
    pub course_title: String,
    /// Identifier of the unit block.
    pub unit_id: String,
    /// Identifier of the chapter holding this unit; empty for a top-level unit.
    pub parent_id: String,
    /// Title of the unit.
    pub title: String,
    /// Optional study amount (`1..=100_000`).
    #[schema(minimum = 1, maximum = 100_000)]
    pub amount: Option<u32>,
    /// Optional unit of study amount (`minutes`, `pages`, or `problems`).
    pub amount_unit: Option<AmountUnit>,
    /// Shared scheduled date (`YYYY-MM-DD`) on the unit, if any.
    pub scheduled_date: Option<String>,
    /// Caller's personal planned date (`YYYY-MM-DD`) on the unit, if any.
    pub my_planned_date: Option<String>,
    /// Effective date (`myPlannedDate` if set, otherwise `scheduledDate`).
    pub effective_date: String,
}

/// Caller's today and overdue incomplete units across enrolled courses in a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceLearningToday {
    /// Today's calendar date (`YYYY-MM-DD`) in the caller's configured time zone (UTC when
    /// set to `auto`).
    pub today: String,
    /// Incomplete units with `effectiveDate == today`, ordered by course creation order
    /// (`createdAt`, `courseId`) and then outline order within each course.
    pub today_units: Vec<LearningTodayItem>,
    /// Incomplete units with `effectiveDate < today`, ordered by `effectiveDate` ascending
    /// (oldest first), then course creation order, then outline order within each course.
    pub overdue_units: Vec<LearningTodayItem>,
}

/// Input item for [`distribute_units`]: a unit identifier and its optional study amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitWeight<'a> {
    /// Identifier of the unit block.
    pub unit_id: &'a str,
    /// Study amount (`1..=100_000`); `None` is treated as weight `1`.
    pub amount: Option<u32>,
}

/// Returns today's calendar date for `now_millis` in `time_zone` (UTC when `time_zone` is
/// `auto` or unknown).
#[must_use]
pub fn today_in_time_zone(now_millis: i64, time_zone: &TimeZone) -> DiaryDate {
    day_at(now_millis, time_zone)
        .and_then(|day| day.parse::<DiaryDate>().ok())
        .or_else(|| DiaryDate::of_utc_millis(now_millis))
        .unwrap_or(DiaryDate::UNIX_EPOCH)
}

/// Returns every calendar date in `start..=target` whose weekday is selected by
/// `study_days`, in ascending chronological order.
#[must_use]
pub fn study_dates_in_range(
    start: DiaryDate,
    target: DiaryDate,
    study_days: StudyDays,
) -> Vec<DiaryDate> {
    if target < start {
        return Vec::new();
    }
    let mut dates = Vec::new();
    let mut current = start;
    while current <= target {
        if study_days.includes_date(current) {
            dates.push(current);
        }
        let Some(next) = current.plus_days(1) else {
            break;
        };
        current = next;
    }
    dates
}

/// Returns the live [`UnitKind::Unit`] nodes of a course in true outline tree order
/// (chapters are traversed in sibling order and expanded to their child units, and
/// top-level units appear at their sibling position; chapters themselves are omitted).
#[must_use]
pub fn outline_units(nodes: &[UnitNode]) -> Vec<&UnitNode> {
    let mut top_level: Vec<&UnitNode> = nodes
        .iter()
        .filter(|node| node.parent_id.is_empty())
        .collect();
    top_level.sort_by(|a, b| (&a.order, &a.id).cmp(&(&b.order, &b.id)));

    let mut children_by_chapter: HashMap<&str, Vec<&UnitNode>> = HashMap::new();
    for node in nodes {
        if !node.parent_id.is_empty() && node.kind == UnitKind::Unit {
            children_by_chapter
                .entry(node.parent_id.as_str())
                .or_default()
                .push(node);
        }
    }
    for children in children_by_chapter.values_mut() {
        children.sort_by(|a, b| (&a.order, &a.id).cmp(&(&b.order, &b.id)));
    }

    let mut ordered = Vec::new();
    let mut seen = HashSet::new();
    for node in top_level {
        match node.kind {
            UnitKind::Unit => {
                if seen.insert(node.id.as_str()) {
                    ordered.push(node);
                }
            }
            UnitKind::Chapter => {
                if let Some(children) = children_by_chapter.get(node.id.as_str()) {
                    for child in children {
                        if seen.insert(child.id.as_str()) {
                            ordered.push(*child);
                        }
                    }
                }
            }
        }
    }

    // Defensive fallback for any live unit whose parent chapter is absent from `nodes`.
    let mut orphans: Vec<&UnitNode> = nodes
        .iter()
        .filter(|node| node.kind == UnitKind::Unit && !seen.contains(node.id.as_str()))
        .collect();
    orphans.sort_by(|a, b| (&a.order, &a.id).cmp(&(&b.order, &b.id)));
    ordered.extend(orphans);
    ordered
}

/// Distributes `units` (in outline order) across the study days in `start..=target`
/// proportionally to each unit's `amount` (default weight `1`), preserving unit order.
///
/// Uses exact `u128` integer arithmetic without floating-point rounding drift. Returns an
/// empty list if `units` is empty or no study day falls in `start..=target`.
#[must_use]
pub fn distribute_units(
    units: &[UnitWeight<'_>],
    start: DiaryDate,
    target: DiaryDate,
    study_days: StudyDays,
) -> Vec<(String, DiaryDate)> {
    if units.is_empty() {
        return Vec::new();
    }
    let days = study_dates_in_range(start, target, study_days);
    if days.is_empty() {
        return Vec::new();
    }
    let day_count = u128::try_from(days.len()).unwrap_or(1);
    let total_weight: u128 = units
        .iter()
        .map(|unit| u128::from(unit.amount.unwrap_or(1).max(1)))
        .sum();
    let denominator = 2 * total_weight;
    let mut assigned = Vec::with_capacity(units.len());
    let mut cumulative: u128 = 0;

    for unit in units {
        let weight = u128::from(unit.amount.unwrap_or(1).max(1));
        // Units smaller than a day's budget map by their midpoint; units larger than a
        // day's budget start on the first day where at least half the day belongs to them.
        let half_step = (weight * day_count).min(total_weight);
        let numerator = 2 * cumulative * day_count + half_step;
        let index =
            usize::try_from((numerator / denominator).min(day_count - 1)).unwrap_or(days.len() - 1);
        assigned.push((unit.unit_id.to_owned(), days[index]));
        cumulative += weight;
    }
    assigned
}

/// Builds a [`CourseScheduleView`] for the calling user from the course's stored schedule,
/// outline nodes, the caller's own progress rows, and `today` (in the caller's time zone).
///
/// Never mutates any stored date: overdue incomplete units remain on their existing dates
/// and are counted in `overdue_count`.
#[must_use]
pub fn build_course_schedule_view(
    schedule: Option<CourseSchedule>,
    nodes: &[UnitNode],
    progress: &[UnitProgress],
    today: DiaryDate,
) -> CourseScheduleView {
    let today_str = today.to_string();
    let progress_by_unit: HashMap<&str, &UnitProgress> = progress
        .iter()
        .map(|row| (row.unit_id.as_str(), row))
        .collect();

    let mut units = Vec::new();
    let mut today_count: u32 = 0;
    let mut overdue_count: u32 = 0;

    for node in outline_units(nodes) {
        let row = progress_by_unit.get(node.id.as_str()).copied();
        let my_planned_date = row.and_then(|p| p.planned_date.clone());
        let completed = row.is_some_and(|p| p.completed_at.is_some());
        let effective_date = my_planned_date
            .clone()
            .or_else(|| node.scheduled_date.clone());
        let overdue = !completed
            && effective_date
                .as_deref()
                .is_some_and(|date| date < today_str.as_str());
        if !completed && effective_date.as_deref() == Some(today_str.as_str()) {
            today_count = today_count.saturating_add(1);
        }
        if overdue {
            overdue_count = overdue_count.saturating_add(1);
        }
        units.push(ScheduledUnit {
            unit_id: node.id.clone(),
            parent_id: node.parent_id.clone(),
            title: node.title.clone(),
            amount: node.amount,
            amount_unit: node.amount_unit,
            scheduled_date: node.scheduled_date.clone(),
            my_planned_date,
            effective_date,
            completed,
            overdue,
        });
    }

    CourseScheduleView {
        schedule,
        today: today_str,
        today_count,
        overdue_count,
        units,
    }
}

/// Enrolled course metadata paired with its outline nodes for building a workspace
/// today/overdue feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CourseOutlineNodes {
    /// Course container identifier.
    pub course_id: String,
    /// The title of the course, as its container has it.
    pub course_title: String,
    /// Outline units and chapters belonging to the course.
    pub nodes: Vec<UnitNode>,
}

/// Computes `today_units` and `overdue_units` for a list of enrolled courses (already in
/// course creation order) and their outline nodes plus the caller's progress map.
#[must_use]
pub fn build_workspace_learning_today(
    courses: &[CourseOutlineNodes],
    progress_by_unit: &BTreeMap<String, UnitProgress>,
    today: DiaryDate,
) -> WorkspaceLearningToday {
    let today_str = today.to_string();
    let mut today_units = Vec::new();
    let mut overdue_units = Vec::new();

    for course in courses {
        for node in outline_units(&course.nodes) {
            let row = progress_by_unit.get(&node.id);
            if row.is_some_and(|p| p.completed_at.is_some()) {
                continue;
            }
            let my_planned_date = row.and_then(|p| p.planned_date.clone());
            let Some(effective_date) = my_planned_date
                .clone()
                .or_else(|| node.scheduled_date.clone())
            else {
                continue;
            };
            let item = LearningTodayItem {
                course_id: course.course_id.clone(),
                course_title: course.course_title.clone(),
                unit_id: node.id.clone(),
                parent_id: node.parent_id.clone(),
                title: node.title.clone(),
                amount: node.amount,
                amount_unit: node.amount_unit,
                scheduled_date: node.scheduled_date.clone(),
                my_planned_date,
                effective_date,
            };
            if item.effective_date == today_str {
                today_units.push(item);
            } else if item.effective_date < today_str {
                overdue_units.push(item);
            }
        }
    }

    // Stable sort preserves course creation order and outline order on equal dates.
    overdue_units.sort_by(|a, b| a.effective_date.cmp(&b.effective_date));

    WorkspaceLearningToday {
        today: today_str,
        today_units,
        overdue_units,
    }
}

fn parse_date_field(refused: &mut Refusals, member: &str, text: &str) -> Option<DiaryDate> {
    if let Ok(date) = text.parse::<DiaryDate>() {
        Some(date)
    } else {
        refused.refuse(member, RefusalReason::InvalidFormat, DATE_INVALID);
        None
    }
}

fn parse_study_days(refused: &mut Refusals, raw: i32) -> Option<StudyDays> {
    u8::try_from(raw)
        .ok()
        .and_then(StudyDays::try_new)
        .or_else(|| {
            refused.refuse("studyDays", RefusalReason::Invalid, STUDY_DAYS_INVALID);
            None
        })
}

fn validate_schedule_window(
    refused: &mut Refusals,
    start: Option<DiaryDate>,
    target: Option<DiaryDate>,
    study_days: Option<StudyDays>,
) {
    let (Some(start), Some(target)) = (start, target) else {
        return;
    };
    if target < start {
        refused.refuse("targetDate", RefusalReason::Invalid, TARGET_BEFORE_START);
        return;
    }
    let span = start
        .civil_date()
        .until(target.civil_date())
        .map_or(i64::MAX, |s| i64::from(s.get_days()));
    if span > MAX_SCHEDULE_SPAN_DAYS {
        refused.refuse("targetDate", RefusalReason::Invalid, SPAN_TOO_LONG);
        return;
    }
    if let Some(days) = study_days
        && !has_study_day_in_range(start, target, days)
    {
        refused.refuse("studyDays", RefusalReason::Invalid, NO_STUDY_DAYS_IN_RANGE);
    }
}

fn has_study_day_in_range(start: DiaryDate, target: DiaryDate, days: StudyDays) -> bool {
    (0..7).any(|offset| {
        start
            .plus_days(offset)
            .is_some_and(|d| d <= target && days.includes_date(d))
    })
}

#[cfg(test)]
mod tests;
