//! Account-wide (`GET /users/me/today`) and single-workspace (`GET /w/{ws}/today`)
//! `'Today'` hub wire types.

use serde::{Deserialize, Serialize};

use crate::course_schedule::LearningTodayItem;
use crate::unit_review::TodayReviewItem;

/// Maximum items returned per section (`cards`, `units`, `reviews`, `meetings`).
pub const TODAY_SECTION_LIMIT: usize = 100;

/// Workspace and organization context attached to each `'Today'` item.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodayWorkspaceMeta {
    /// Workspace identifier.
    pub workspace_id: String,
    /// Workspace display name (empty for a personal space's unnamed workspace).
    pub workspace_name: String,
    /// Organization identifier.
    pub org_id: String,
    /// Organization display name (empty for a personal space).
    pub org_name: String,
    /// `true` when this workspace is the caller's personal space (`OrgKind::Personal`).
    pub is_personal: bool,
}

/// An incomplete card assigned to the caller that is overdue, due today, or due soon.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodayCardItem {
    /// Card block identifier.
    pub card_id: String,
    /// Board-scoped card number (`#12`), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card_number: Option<u32>,
    /// Card title.
    pub title: String,
    /// Card icon, if any.
    pub icon: String,
    /// Board container identifier.
    pub board_id: String,
    /// The title of the board the card is on.
    pub board_title: String,
    /// The icon of the board the card is on; empty if it has none.
    pub board_icon: String,
    /// Effective due calendar day (`YYYY-MM-DD`) in the caller's time zone.
    pub due_date: String,
    /// Exact epoch milliseconds when the due property holds a timestamp; `null` for a
    /// calendar date.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due_at_millis: Option<i64>,
    /// Days relative to `today`: `> 0` overdue, `0` due today, `< 0` due tomorrow / soon.
    pub overdue_days: i32,
    /// Workspace metadata for cross-workspace navigation.
    pub workspace: TodayWorkspaceMeta,
}

/// An enrolled course unit scheduled for today or overdue, paired with its workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodayUnitEntry {
    /// Scheduled unit details.
    #[serde(flatten)]
    pub unit: LearningTodayItem,
    /// Workspace metadata.
    pub workspace: TodayWorkspaceMeta,
}

/// A pending spaced-repetition unit review due today or overdue, paired with its workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodayReviewEntry {
    /// Due review details (including `suggestedNextIntervalDays` and `reviewIntervals`).
    #[serde(flatten)]
    pub review: TodayReviewItem,
    /// Workspace metadata.
    pub workspace: TodayWorkspaceMeta,
}

/// An upcoming study meeting in an enrolled course.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TodayMeetingItem {
    /// Meeting identifier.
    pub meeting_id: String,
    /// Course container identifier.
    pub container_id: String,
    /// Current course title.
    pub course_title: String,
    /// Meeting title.
    pub title: String,
    /// Scheduled start time in milliseconds since the Unix epoch.
    pub starts_at: i64,
    /// Optional scheduled end time in milliseconds since the Unix epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<i64>,
    /// Location text or link.
    pub location: String,
    /// Workspace metadata.
    pub workspace: TodayWorkspaceMeta,
}

/// Single-workspace `'Today'` summary (`GET /api/v1/w/{workspaceId}/today`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each section independently reports whether additional items were truncated at 100"
)]
pub struct WorkspaceToday {
    /// Caller's current calendar day (`YYYY-MM-DD`).
    pub today: String,
    /// Metadata of the queried workspace.
    pub workspace: TodayWorkspaceMeta,
    /// Incomplete assigned cards overdue, due today, or due soon (up to 100).
    pub cards: Vec<TodayCardItem>,
    /// Whether more than 100 due/overdue cards matched.
    pub has_more_cards: bool,
    /// Enrolled course units scheduled for today or overdue (up to 100).
    pub units: Vec<TodayUnitEntry>,
    /// Whether more than 100 units matched.
    pub has_more_units: bool,
    /// Pending personal reviews due today or overdue (up to 100).
    pub reviews: Vec<TodayReviewEntry>,
    /// Whether more than 100 reviews matched.
    pub has_more_reviews: bool,
    /// Upcoming course meetings starting soon or today (up to 100).
    pub meetings: Vec<TodayMeetingItem>,
    /// Whether more than 100 meetings matched.
    pub has_more_meetings: bool,
}

/// Account-wide `'Today'` hub summary (`GET /api/v1/users/me/today`).
///
/// Diary entries are intentionally absent so the server never chooses a diary day on the
/// caller's behalf; the web client renders personal diary streaks via `getDiaryRuns`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each section independently reports whether additional items were truncated at 100"
)]
pub struct AccountToday {
    /// Caller's current calendar day (`YYYY-MM-DD`).
    pub today: String,
    /// Number of workspaces excluded because the current session has not satisfied their
    /// SSO requirement or personal-space password confirmation.
    pub locked_workspace_count: u32,
    /// Incomplete assigned cards overdue, due today, or due soon across open workspaces.
    pub cards: Vec<TodayCardItem>,
    /// Whether more than 100 due/overdue cards matched across open workspaces.
    pub has_more_cards: bool,
    /// Enrolled course units scheduled for today or overdue across open workspaces.
    pub units: Vec<TodayUnitEntry>,
    /// Whether more than 100 units matched across open workspaces.
    pub has_more_units: bool,
    /// Pending personal reviews due today or overdue across open workspaces.
    pub reviews: Vec<TodayReviewEntry>,
    /// Whether more than 100 reviews matched across open workspaces.
    pub has_more_reviews: bool,
    /// Upcoming course meetings across open workspaces.
    pub meetings: Vec<TodayMeetingItem>,
    /// Whether more than 100 meetings matched across open workspaces.
    pub has_more_meetings: bool,
}
