use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "api-schema")]
use utoipa::ToSchema;

pub type IsoDate = String;

#[derive(Debug, Clone)]
pub struct GanttTaskInput {
    pub id: Uuid,
    pub title: String,
    pub start: Option<IsoDate>,
    pub due: Option<IsoDate>,
    pub milestone: bool,
}

#[derive(Debug, Clone)]
pub struct GanttLinkInput {
    pub blocker_id: Uuid,
    pub blocked_id: Uuid,
    pub link_type: LinkType,
    pub lag_days: i32,
}

/// Dependency type: finish-to-start, start-to-start or finish-to-finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
#[cfg_attr(feature = "api-schema", schema(as = GanttLinkType))]
#[serde(rename_all = "UPPERCASE")]
pub enum LinkType {
    Fs,
    Ss,
    Ff,
}

impl LinkType {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "FS" => Some(Self::Fs),
            "SS" => Some(Self::Ss),
            "FF" => Some(Self::Ff),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ScheduleInference {
    None,
    #[serde(rename = "from-due")]
    FromDue,
    #[serde(rename = "from-start")]
    FromStart,
    Swapped,
}

#[derive(Debug, Clone)]
pub struct ScheduledTask {
    pub id: Uuid,
    pub title: String,
    pub start: IsoDate,
    pub end: IsoDate,
    pub milestone: bool,
    pub inferred: ScheduleInference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomLevel {
    Day,
    Week,
    Month,
    Quarter,
}

impl ZoomLevel {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "day" => Some(Self::Day),
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            "quarter" => Some(Self::Quarter),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
            Self::Quarter => "quarter",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackMode {
    Rows,
    Overlap,
}

impl PackMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "rows" => Some(Self::Rows),
            "overlap" => Some(Self::Overlap),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rows => "rows",
            Self::Overlap => "overlap",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TimeScale {
    pub zoom: ZoomLevel,
    pub start: IsoDate,
    pub end: IsoDate,
    pub px_per_day: f64,
}

#[derive(Debug, Clone)]
pub struct LaidOutBar {
    pub id: Uuid,
    pub lane: i32,
    pub x: f64,
    pub width: f64,
    pub milestone: bool,
    pub inferred: ScheduleInference,
}

#[derive(Debug, Clone)]
pub struct WorkCalendar {
    pub weekend: Vec<u8>,
    pub holidays: std::collections::HashSet<IsoDate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct ScaleTickOutput {
    pub date: IsoDate,
    pub x: f64,
    pub width: f64,
    pub label: String,
    #[serde(rename = "offDuty")]
    pub off_duty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
pub struct MonthBandOutput {
    pub key: String,
    pub label: String,
    pub x: f64,
    pub width: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct GanttBarOutput {
    pub id: String,
    pub lane: i32,
    pub x: f64,
    pub width: f64,
    pub milestone: bool,
    pub inferred: ScheduleInference,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct GanttScaleOutput {
    pub zoom: String,
    pub start: IsoDate,
    pub end: IsoDate,
    pub px_per_day: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct GanttLayoutItemOutput {
    pub id: String,
    pub title: String,
    pub number: i32,
    pub status_id: String,
    pub priority: String,
    pub assignee_ids: Vec<String>,
    pub start_date: Option<IsoDate>,
    pub due_date: Option<IsoDate>,
    /// RFC 3339 UTC with milliseconds, or microseconds when the stored value
    /// has finer precision; send it back unchanged as `expectedDates.dueAt`.
    pub due_at: Option<String>,
    pub start: IsoDate,
    pub end: IsoDate,
    pub milestone: bool,
    pub inferred: ScheduleInference,
}

/// A dependency between two returned items. The server alone enforces it: a
/// PATCH that breaks it is refused with `dependency_contradiction`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct GanttLinkOutput {
    pub blocker_id: String,
    pub blocked_id: String,
    #[serde(rename = "type")]
    pub link_type: LinkType,
    /// Working days the blocked task's date must trail the blocker's date by;
    /// weekends and workspace holidays do not count.
    pub lag_days: i32,
}

/// Non-working days, as the server applies them to `columns[].offDuty` and
/// to dependency lag.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct GanttCalendarOutput {
    /// Weekdays that are never working days, 0 = Sunday .. 6 = Saturday.
    pub weekend: Vec<u8>,
    /// Workspace holidays within `scale.start..=scale.end`, ascending.
    pub holidays: Vec<IsoDate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api-schema", derive(ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct GanttLayoutOutput {
    pub truncated: bool,
    pub items: Vec<GanttLayoutItemOutput>,
    /// At least Edit on the project and the project not archived, read in the
    /// same snapshot as `items`. A display hint: PATCH re-checks both under
    /// the project row lock. Ignores API-token scopes; PATCH also needs
    /// `tasks.write`.
    pub can_edit: bool,
    /// Dependencies whose both ends are in `items`, ascending
    /// `(blockerId, blockedId)` and capped at 2048 (over the cap, the lowest
    /// pairs).
    pub links: Vec<GanttLinkOutput>,
    /// Dependencies among `items` before the cap.
    pub link_total: i32,
    pub calendar: GanttCalendarOutput,
    pub scale: GanttScaleOutput,
    pub lane_height: i32,
    pub lane_count: i32,
    pub pack: String,
    pub bars: Vec<GanttBarOutput>,
    pub path_total: i32,
    /// Compact paths: [blockerIndex, blockedIndex, x1, y1, x2, y2, ...]
    pub paths: Vec<Vec<i64>>,
    pub columns: Vec<ScaleTickOutput>,
    pub month_bands: Vec<MonthBandOutput>,
    pub width: f64,
    pub height: f64,
    pub overflow: Vec<String>,
    pub dropped: Vec<String>,
}
